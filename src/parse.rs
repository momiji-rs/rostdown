//! Block + span parsing into a small element tree. Anything outside the
//! implemented subset returns `Error::Declined(reason)` — the whole
//! document is parsed before any HTML is emitted, so a decline can never
//! leave partial output.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::{Error, Options, typography};

/// The element tree is a *borrowed, flat-arena* AST.
///
/// Borrowed: every text field is a `Cow<'a, str>` that borrows directly
/// from the input `src` whenever the rendered bytes are identical to the
/// source bytes (the common case — prose, code, hrefs), and only
/// materializes an owned `String` when a typography rewrite
/// (`'`/`"`/`-`/`.`) or backslash escape mutates a char. The lifetime
/// `'a` is tied to `src` inside `to_html`, so the AST never outlives the
/// document it borrows from.
///
/// Flat-arena: there are no per-node `Vec`s. Spans, blocks and list items
/// each live in one growable arena owned by [`Ast`], and node lists are
/// *sibling-linked* — a list is the chain reached by following `next`
/// from a start index, and a composite's children are the chain from its
/// `first_child` index. The recursive inline parser interleaves a
/// composite's children with the siblings that follow it, so a single
/// contiguous range can't describe a list; the `next` link can. The
/// arenas are three `Vec`s grown by `push`, so one render allocates a
/// handful of buffers (amortized) instead of ~855 little node `Vec`s.
///
/// Backtracking: the inline parser speculatively recurses on an emphasis
/// open and reverts (`parse_spans_until` → `None`) when no close is
/// found. Each speculative recursion records `ast.spans.len()` first and
/// `truncate`s back to it on revert, so abandoned nodes never leak into
/// the arena or the output.
/// Link reference definitions: normalized id → (url, optional title),
/// the url/title borrowing from `src`.
type LinkDefs<'a> = HashMap<String, (&'a str, Option<&'a str>)>;

/// Span IAL attributes (`[t](u){:.c}`) keyed by span index — a side table
/// so the hot per-span `SpanNode` stays small. Empty for most docs.
type SpanIals<'a> = HashMap<u32, Vec<(Cow<'a, str>, String)>>;

#[derive(Debug, Default)]
pub(crate) struct Ast<'a> {
    pub(crate) blocks: Vec<BlockNode<'a>>,
    pub(crate) spans: Vec<SpanNode<'a>>,
    pub(crate) items: Vec<ItemNode<'a>>,
    /// `[id]: url "title"` definitions, collected in a pre-pass and keyed
    /// by normalized id. Empty for the common doc with none.
    pub(crate) link_defs: LinkDefs<'a>,
    /// Span IAL attributes keyed by span index (see [`SpanIals`]).
    pub(crate) span_ials: SpanIals<'a>,
}

impl<'a> Ast<'a> {
    fn new() -> Self {
        Ast::default()
    }

    /// Pre-size the arenas from the source length so the hot top-level
    /// parse rarely re-grows (each regrow is a realloc + memcpy of the
    /// whole arena — the dominant cost once per-node `Vec`s are gone).
    /// Heuristics from the bench corpus: spans dominate (~1 node / 12 B
    /// of source — prose plus markup splits), blocks ~1 / 40 B, items
    /// far rarer. Generous but bounded; a tiny doc still costs three
    /// small `Vec`s, an over-estimate just over-reserves once.
    fn with_capacity_for(src_len: usize) -> Self {
        Ast {
            blocks: Vec::with_capacity(src_len / 40 + 8),
            spans: Vec::with_capacity(src_len / 12 + 16),
            items: Vec::with_capacity(src_len / 256 + 4),
            link_defs: HashMap::new(),
            span_ials: HashMap::new(),
        }
    }

    /// Push a span node and return its index. Not linked yet — the
    /// caller (a [`Chain`]) sets `next`.
    #[inline]
    fn push_span(&mut self, kind: SpanKind<'a>) -> u32 {
        let idx = self.spans.len() as u32;
        self.spans.push(SpanNode { kind, next: None });
        idx
    }

    #[inline]
    fn push_block(&mut self, kind: BlockKind<'a>) -> u32 {
        let idx = self.blocks.len() as u32;
        self.blocks.push(BlockNode {
            kind,
            next: None,
            ial: Vec::new(),
        });
        idx
    }

    #[inline]
    fn push_item(&mut self, item: ItemNode<'a>) -> u32 {
        let idx = self.items.len() as u32;
        self.items.push(item);
        idx
    }
}

/// One block in the flat arena plus its sibling link. `next == None`
/// terminates a block list.
#[derive(Debug)]
pub(crate) struct BlockNode<'a> {
    pub(crate) kind: BlockKind<'a>,
    pub(crate) next: Option<u32>,
    /// Block IAL attributes (`{:.class}` etc.) in kramdown's insertion
    /// order — `(name, value)`, with classes accumulated under `class`.
    /// Empty for the common block with none.
    pub(crate) ial: Vec<(Cow<'a, str>, String)>,
}

#[derive(Debug)]
pub(crate) enum BlockKind<'a> {
    /// A run of blank lines between blocks (renders as one `\n`).
    Blank,
    /// `raw` is the unparsed heading text — kramdown CORE derives
    /// `auto_ids` slugs from it. `span_text` is the parsed-tree text
    /// (typography applied, link text included, markup gone) — the GFM
    /// parser's `generate_gfm_header_id` input. `spans` is the index of
    /// the first child span (or `None` for an empty heading).
    Heading {
        level: u8,
        raw: Cow<'a, str>,
        span_text: Cow<'a, str>,
        spans: Option<u32>,
    },
    /// Paragraph: index of the first child span (`None` ⇒ empty).
    Para(Option<u32>),
    /// Tight list (no blank lines inside) — `items` is the index of the
    /// first [`ItemNode`]. Items are span runs plus an optional trailing
    /// nested child list; lazy continuations join the item's spans with a
    /// literal newline (kramdown's verbatim line joining).
    List {
        ordered: bool,
        /// Loose list (a blank line separates items): each item's content
        /// is wrapped in `<p>` (kramdown). Tight lists stay `<li>text</li>`.
        loose: bool,
        items: Option<u32>,
    },
    /// Blockquote: index of the first child block (`None` ⇒ empty).
    Quote(Option<u32>),
    Code {
        lang: Option<Cow<'a, str>>,
        text: Cow<'a, str>,
    },
    /// A raw HTML block, already re-serialized to match kramdown's HTML
    /// converter (see [`crate::html_block`]). Emitted verbatim.
    RawHtml(String),
    Hr,
    /// A kramdown/GFM pipe table in the common shape: an optional header
    /// row (when a separator line underlines it), per-column alignment,
    /// and body rows. Each cell is the span-head of its parsed content.
    /// Tables outside this shape (multiple bodies, footers, raw-HTML
    /// cells, ragged rows) decline so output stays byte-identical.
    Table {
        aligns: Vec<Align>,
        header: Option<Vec<Option<u32>>>,
        body: Vec<Vec<Option<u32>>>,
    },
}

/// Per-column text alignment from a table's separator line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    None,
    Left,
    Center,
    Right,
}

/// One tight-list item in the flat arena. `spans` is the first child
/// span; `child` is the first item of an optional trailing nested list;
/// `next` links to the following sibling item.
#[derive(Debug)]
pub(crate) struct ItemNode<'a> {
    pub(crate) spans: Option<u32>,
    /// Trailing nested list: `(ordered, first_item_index)`. Tight items
    /// carry at most text-then-one-child in our subset; anything richer
    /// (blank lines, content after the child) declines first.
    pub(crate) child: Option<(bool, Option<u32>)>,
    pub(crate) next: Option<u32>,
    /// Lifetime tie: an `ItemNode` with all-`None` index fields would
    /// otherwise be `'static`. Keeps `'a` bound to `src`.
    _marker: std::marker::PhantomData<&'a str>,
}

/// One span in the flat arena plus its sibling link. `next == None`
/// terminates a span chain (a span run, or a composite's children).
#[derive(Debug)]
pub(crate) struct SpanNode<'a> {
    pub(crate) kind: SpanKind<'a>,
    pub(crate) next: Option<u32>,
}

#[derive(Debug)]
pub(crate) enum SpanKind<'a> {
    /// Raw text (typography + escaping applied at conversion). Borrows a
    /// pristine `src` slice unless a rewrite forced materialization.
    Text(Cow<'a, str>),
    /// Verbatim HTML emitted without escaping — an inline raw-HTML element
    /// already re-serialized to match kramdown (see [`crate::html_block`]).
    Raw(Cow<'a, str>),
    /// Emphasis: index of the first child span.
    Em(Option<u32>),
    /// GFM strikethrough (`~~…~~`): index of the first child span.
    Del(Option<u32>),
    /// Strong: index of the first child span.
    Strong(Option<u32>),
    Code(Cow<'a, str>),
    Link {
        /// Index of the first child span.
        spans: Option<u32>,
        href: Cow<'a, str>,
        /// Optional `title="…"` from `[text](url "title")` / `'title'`.
        /// HTML-attr-escaped at conversion; `None` for the common
        /// no-title link.
        title: Option<Cow<'a, str>>,
    },
    /// `![alt](src "title")` → `<img>`. `alt` is the raw bracket text
    /// (kramdown does not parse markup inside it); all three are
    /// HTML-attr-escaped at conversion.
    Image {
        src: Cow<'a, str>,
        alt: Cow<'a, str>,
        title: Option<Cow<'a, str>>,
    },
}

/// Builds a sibling-linked chain in one of the arenas: tracks the first
/// and last index pushed so each new node's predecessor gets its `next`
/// patched. `first()` returns the chain head for storing in a parent.
///
/// `Link` is the per-arena patch operation — given a node index, set its
/// `next` field — so one `Chain` type serves spans, blocks and items.
struct Chain {
    first: Option<u32>,
    last: Option<u32>,
}

impl Chain {
    #[inline]
    fn new() -> Self {
        Chain { first: None, last: None }
    }

    /// Append `idx` to the chain, patching the previous tail's `next`
    /// via `set_next`. The pushed node's own `next` must already be
    /// `None` (the arena `push_*` helpers guarantee this).
    #[inline]
    fn link(&mut self, idx: u32, set_next: impl FnOnce(u32, u32)) {
        match self.last {
            None => self.first = Some(idx),
            Some(prev) => set_next(prev, idx),
        }
        self.last = Some(idx);
    }

    #[inline]
    fn first(&self) -> Option<u32> {
        self.first
    }
}

fn declined(what: &'static str) -> Error {
    Error::Declined(what)
}

/// Split `src` on `\n` into line slices — byte-identical to
/// `src.split('\n').collect()` (a trailing `\n` yields a final empty
/// element) but with a tight byte scan instead of std's char-pattern
/// searcher.
fn split_lines(src: &str) -> Vec<&str> {
    let bytes = src.as_bytes();
    // Heuristic capacity (~32 B/line) so the Vec rarely regrows, without
    // a second pass to count newlines exactly.
    let mut out = Vec::with_capacity(src.len() / 32 + 8);
    let mut start = 0;
    // SWAR memchr1 finds each `\n` a word at a time instead of scanning
    // byte-by-byte (line splitting was the top parse self-time).
    while let Some(off) = crate::scan::memchr1(&bytes[start..], b'\n') {
        let nl = start + off;
        out.push(&src[start..nl]);
        start = nl + 1;
    }
    out.push(&src[start..]);
    out
}

/// Parse `src` into a flat-arena [`Ast`]. The returned `root` is the
/// index of the first top-level block (`None` for an empty document);
/// the converter walks the arenas from there.
pub(crate) fn parse<'a>(src: &'a str, opts: &Options) -> Result<(Ast<'a>, Option<u32>), Error> {
    let lines: Vec<&'a str> = split_lines(src);
    // A trailing "\n" yields one empty last element — drop it so it
    // doesn't read as a blank line.
    let lines = match lines.last() {
        Some(&"") => &lines[..lines.len() - 1],
        _ => &lines[..],
    };
    let mut ast = Ast::with_capacity_for(src.len());
    // Pre-pass: lift block-level link reference definitions out of the
    // stream so the surrounding blank lines collapse the way kramdown's
    // do, and so `[text][id]` / `[text]` resolve during span parsing.
    let (defs, def_mask) = collect_link_defs(lines)?;
    let root = if defs.is_empty() {
        parse_blocks(&mut ast, src, lines, opts)?
    } else {
        ast.link_defs = defs;
        let filtered: Vec<&'a str> = lines
            .iter()
            .zip(&def_mask)
            .filter_map(|(&l, &is_def)| (!is_def).then_some(l))
            .collect();
        parse_blocks(&mut ast, src, &filtered, opts)?
    };
    Ok((ast, root))
}

/// Byte offset of slice `s` within its backing string `src`. `s` MUST be
/// a sub-slice of `src` (true for every line / span slice we derive from
/// the input). Safe pointer arithmetic — no dereference — used to
/// reconstruct a contiguous `src` range from two of its sub-slices so a
/// multi-line paragraph / fenced-code body can be borrowed as one slice
/// instead of joined into a fresh `String`.
#[inline]
fn offset_in(src: &str, s: &str) -> usize {
    s.as_ptr() as usize - src.as_ptr() as usize
}

/// ASCII whitespace per `char::is_whitespace` (note: includes VT `0x0B`,
/// which `u8::is_ascii_whitespace` omits).
#[inline]
fn ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// `line.trim().is_empty()` with an ASCII fast path — most lines decide on
/// the first byte (a prose letter ⇒ not blank). Falls back to the precise
/// Unicode-aware check only when a non-ASCII byte is reached.
#[inline]
fn is_blank(line: &str) -> bool {
    for (i, &b) in line.as_bytes().iter().enumerate() {
        if b >= 0x80 {
            return line[i..].trim_start().is_empty();
        }
        if !ascii_ws(b) {
            return false;
        }
    }
    true
}

/// `str::trim_start` with an ASCII fast path (non-ASCII boundary ⇒ defer
/// to the Unicode-aware trim, which may strip more, e.g. NBSP).
#[inline]
fn trim_start_ws(s: &str) -> &str {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] >= 0x80 {
            return s[i..].trim_start();
        }
        if !ascii_ws(b[i]) {
            break;
        }
        i += 1;
    }
    &s[i..]
}

/// `str::trim` (both ends) with an ASCII fast path; defers to the precise
/// Unicode trim when a trim boundary lands on a non-ASCII byte.
#[inline]
fn trim_ws(s: &str) -> &str {
    let b = s.as_bytes();
    let mut start = 0;
    while start < b.len() && b[start] < 0x80 && ascii_ws(b[start]) {
        start += 1;
    }
    let mut end = b.len();
    while end > start && b[end - 1] < 0x80 && ascii_ws(b[end - 1]) {
        end -= 1;
    }
    if (start < b.len() && b[start] >= 0x80) || (end > start && b[end - 1] >= 0x80) {
        return s.trim(); // non-ASCII at a boundary: be precise
    }
    &s[start..end]
}

/// `str::trim_end` with an ASCII fast path (non-ASCII boundary ⇒ defer to
/// the Unicode-aware trim, which may strip more).
#[inline]
fn trim_end_ws(s: &str) -> &str {
    let b = s.as_bytes();
    let mut end = b.len();
    while end > 0 && b[end - 1] < 0x80 && ascii_ws(b[end - 1]) {
        end -= 1;
    }
    if end > 0 && b[end - 1] >= 0x80 {
        return s.trim_end();
    }
    &s[..end]
}

/// Insert or update `key` in an insertion-ordered attribute list.
fn set_attr<'a>(attrs: &mut Vec<(Cow<'a, str>, String)>, key: Cow<'a, str>, val: String) {
    if let Some(e) = attrs.iter_mut().find(|(k, _)| k.as_ref() == key.as_ref()) {
        e.1 = val;
    } else {
        attrs.push((key, val));
    }
}

/// Parse a block IAL's inner content (between `{:` and `}`) into kramdown's
/// insertion-ordered attribute list: `.class` (dot- or space-separated)
/// accumulates under `class`, `#id` sets `id`, and `key="v"` / `key='v'` /
/// `key=v` set arbitrary attributes. `None` for a bare name (an ALD
/// reference / `{:toc}`) or a malformed token — those decline.
fn parse_ial(content: &str) -> Option<Vec<(Cow<'_, str>, String)>> {
    let mut attrs: Vec<(Cow<'_, str>, String)> = Vec::new();
    let b = content.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && matches!(b[i], b' ' | b'\t') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        match b[i] {
            b'.' => {
                let s = i + 1;
                let mut j = s;
                while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'.' | b'#') {
                    j += 1;
                }
                if j == s {
                    return None;
                }
                let cls = &content[s..j];
                if let Some(e) = attrs.iter_mut().find(|(k, _)| k.as_ref() == "class") {
                    e.1.push(' ');
                    e.1.push_str(cls);
                } else {
                    attrs.push((Cow::Borrowed("class"), cls.to_string()));
                }
                i = j;
            }
            b'#' => {
                let s = i + 1;
                let mut j = s;
                while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'.' | b'#') {
                    j += 1;
                }
                if j == s {
                    return None;
                }
                set_attr(&mut attrs, Cow::Borrowed("id"), content[s..j].to_string());
                i = j;
            }
            _ => {
                let s = i;
                let mut j = i;
                while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'=') {
                    j += 1;
                }
                if j == s || j >= b.len() || b[j] != b'=' {
                    return None; // bare name (ALD ref / toc) or malformed
                }
                let key = &content[s..j];
                let vs = j + 1;
                let (val, next) = if matches!(b.get(vs), Some(b'"' | b'\'')) {
                    let q = b[vs];
                    let inner = &content[vs + 1..];
                    let end = inner.bytes().position(|c| c == q)?;
                    (inner[..end].to_string(), vs + 1 + end + 1)
                } else {
                    let mut k = vs;
                    while k < b.len() && !matches!(b[k], b' ' | b'\t') {
                        k += 1;
                    }
                    (content[vs..k].to_string(), k)
                };
                set_attr(&mut attrs, Cow::Borrowed(key), val);
                i = next;
            }
        }
    }
    if attrs.is_empty() { None } else { Some(attrs) }
}

fn parse_blocks<'a>(
    ast: &mut Ast<'a>,
    src: &'a str,
    lines: &[&'a str],
    opts: &Options,
) -> Result<Option<u32>, Error> {
    let mut chain = Chain::new();
    // Push `kind` into the block arena and link it as the next sibling.
    // A leading block IAL (`{:.note}` on its own line directly BEFORE its
    // block) is buffered here and applied to the next attachable block.
    let mut pending_ial: Vec<(Cow<'a, str>, String)> = Vec::new();
    macro_rules! emit_block {
        ($kind:expr) => {{
            let idx = ast.push_block($kind);
            if !pending_ial.is_empty()
                && matches!(
                    ast.blocks[idx as usize].kind,
                    BlockKind::Para(_)
                        | BlockKind::Heading { .. }
                        | BlockKind::List { .. }
                        | BlockKind::Quote(_)
                )
            {
                ast.blocks[idx as usize].ial = std::mem::take(&mut pending_ial);
            }
            chain.link(idx, |p, n| ast.blocks[p as usize].next = Some(n));
        }};
    }
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if is_blank(line) {
            // Collapse a blank run into one Blank between blocks.
            while i < lines.len() && is_blank(lines[i]) {
                i += 1;
            }
            emit_block!(BlockKind::Blank);
            continue;
        }

        // A block IAL line (`{:.class}` …) immediately after a paragraph,
        // heading, or list attaches its attributes to that block. Span
        // IALs, ALDs, `{::}` extensions, `{:toc}`, IALs after a blank or
        // after other block types are out of subset and fall through to
        // decline.
        if line.trim_start_matches([' ', '\t']).starts_with("{:")
            && !line.trim_start_matches([' ', '\t']).starts_with("{::")
            && line.len() - line.trim_start_matches(' ').len() <= 3
        {
            let t = line.trim_matches([' ', '\t']);
            if let Some(inner) = t.strip_prefix("{:").and_then(|s| s.strip_suffix('}'))
                && let Some(attrs) = parse_ial(inner)
            {
                // Trailing form: `block\n{:.x}` attaches to the PRECEDING
                // block.
                if let Some(last) = chain.last
                    && matches!(
                        ast.blocks[last as usize].kind,
                        BlockKind::Para(_)
                            | BlockKind::Heading { .. }
                            | BlockKind::List { .. }
                            | BlockKind::Quote(_)
                    )
                    && ast.blocks[last as usize].ial.is_empty()
                {
                    ast.blocks[last as usize].ial = attrs;
                    i += 1;
                    continue;
                }
                // Leading form: `{:.x}\nblock` attaches to the FOLLOWING
                // block (kramdown allows a block IAL on either side). Buffer
                // it; `emit_block!` applies it to the next attachable block.
                if pending_ial.is_empty()
                    && line_starts_attachable_block(lines, i + 1, opts)
                {
                    pending_ial = attrs;
                    i += 1;
                    continue;
                }
            }
        }

        // A top-level pipe table. Heading/list/quote/fence take precedence
        // (handled inside); a pipe-shaped run that isn't a clean table
        // returns None and falls through to the pipe-decline below.
        if let Some((kind, consumed)) = try_parse_table(ast, lines, i, opts)? {
            emit_block!(kind);
            i += consumed;
            continue;
        }

        // A raw HTML block opening (column 0) with a block-level element. We
        // re-serialize the supported subset to match kramdown's GFM-parser
        // HTML handling; anything else falls through to the html-block
        // decline. Gated to GFM (the gem profile): kramdown's CORE parser
        // normalizes HTML-block whitespace differently, so under core we
        // keep declining rather than mis-render.
        if opts.gfm && crate::html_block::starts_html_block(line) {
            let off = offset_in(src, line);
            if let Some((html, consumed)) = crate::html_block::serialize(&src[off..]) {
                let lines_used = 1 + src[off..off + consumed].bytes().filter(|&c| c == b'\n').count();
                emit_block!(BlockKind::RawHtml(html));
                i += lines_used;
                continue;
            }
        }

        // A top-level table pipe that `try_parse_table` couldn't render as a
        // clean table. kramdown still makes a table iff EVERY line of the
        // block is a row; reproduce that decline. Otherwise the block is a
        // plain paragraph with literal pipes — fall through to the paragraph
        // path (NOT a decline), matching kramdown.
        if has_table_pipe(trim_start_ws(line)) && block_all_pipe(lines, i, opts) {
            return Err(declined("table"));
        }

        // Indented code block: a run of ≥4-space-indented lines at a block
        // boundary (a line reaching here is a fresh block — a continuation
        // would have been absorbed by the previous paragraph). Strip exactly
        // four spaces per line; interior blank lines are kept when more
        // indented code follows, trailing blanks end the block. Content is
        // emitted as a `Code` block (escaped at render, like a fence). Gated
        // to GFM (the gem profile): kramdown's CORE parser also takes
        // non-indented LAZY continuations into the code block, which we don't
        // model — so under core we keep declining.
        if opts.gfm && line.starts_with("    ") {
            let mut text = String::new();
            let mut j = i;
            let mut pending_blanks = 0usize;
            while j < lines.len() {
                let l = lines[j];
                if is_blank(l) {
                    pending_blanks += 1;
                    j += 1;
                    continue;
                }
                if !l.starts_with("    ") {
                    break;
                }
                for _ in 0..pending_blanks {
                    text.push('\n');
                }
                pending_blanks = 0;
                text.push_str(&l[4..]);
                text.push('\n');
                j += 1;
            }
            // `j` advanced past the trailing blanks too; rewind so they
            // remain blank separators between blocks.
            let consumed = j - pending_blanks;
            emit_block!(BlockKind::Code {
                lang: None,
                text: Cow::Owned(text),
            });
            i = consumed;
            continue;
        }

        decline_block_scan(line)?;

        // ATX heading.
        if let Some(rest) = line.strip_prefix('#') {
            let mut level = 1u8;
            let mut rest = rest;
            while let Some(r) = rest.strip_prefix('#') {
                level += 1;
                rest = r;
            }
            if level <= 6 && rest.starts_with([' ', '\t']) {
                // kramdown's ATX header is `#{1,6}[ \t]+(text)`: all leading
                // whitespace after the hashes is consumed.
                let text = rest.trim_start_matches([' ', '\t']);
                // kramdown's header-id shorthand `… {#id}` sets the id and is
                // stripped (checked on the ws-trimmed text BEFORE stripping
                // closing `#`s — a `{#id}` followed by `###` is literal).
                // Otherwise strip optional trailing hashes as usual.
                let text = trim_end_ws(text);
                let (text, explicit_id) = match extract_header_id(text) {
                    Some((stripped, id)) => (stripped, Some(id)),
                    None => (trim_end_ws(text.trim_end_matches('#')), None),
                };
                let spans = parse_spans(ast, text)?;
                // span_text is the concatenation of child-span texts.
                // The overwhelmingly common heading is plain prose — a
                // single borrowed `Text` span — so reuse that slice
                // instead of allocating; only mixed-markup headings
                // concatenate into an owned `String`.
                let span_text: Cow<'a, str> = match spans {
                    Some(first)
                        if ast.spans[first as usize].next.is_none()
                            && matches!(ast.spans[first as usize].kind, SpanKind::Text(_)) =>
                    {
                        match &ast.spans[first as usize].kind {
                            SpanKind::Text(t) => t.clone(),
                            _ => unreachable!(),
                        }
                    }
                    _ => {
                        let mut s = String::new();
                        spans_raw_text(ast, spans, &mut s);
                        Cow::Owned(s)
                    }
                };
                // GFM slugs we can't reproduce exactly (Unicode word
                // classes outside our supported set, empty results)
                // decline rather than risk a wrong id. The converter
                // builds the real slug later, so the parser only needs
                // the validity bit — a non-allocating check. Skipped when an
                // explicit `{#id}` supplied the id (no auto-id needed).
                if explicit_id.is_none()
                    && opts.gfm
                    && opts.auto_ids
                    && !crate::html::gfm_slug_ok(&span_text)
                {
                    return Err(declined("heading-gfm-slug"));
                }
                emit_block!(BlockKind::Heading {
                    level,
                    // `text` is a trimmed sub-slice of the heading line —
                    // borrow it directly.
                    raw: Cow::Borrowed(text),
                    span_text,
                    spans,
                });
                // An explicit `{#id}` becomes the heading's id (after any
                // leading-IAL classes); kramdown does NOT dedup explicit ids,
                // and the renderer suppresses the auto-id when an id is set.
                if let Some(id) = explicit_id {
                    let hd = chain.last.expect("heading just emitted") as usize;
                    ast.blocks[hd].ial.push((Cow::Borrowed("id"), id.to_string()));
                }
                i += 1;
                continue;
            }
            return Err(declined("atx-heading-shape"));
        }

        // Horizontal rule: 3+ of the same marker, only that marker +
        // spaces on the line.
        if is_hr(line) {
            emit_block!(BlockKind::Hr);
            i += 1;
            continue;
        }

        // Fenced code block. The opening fence is a run of ≥3 backticks
        // (GFM) or tildes; kramdown closes it with a run of the SAME char
        // at least as long, carrying no info string — so a ```` ```` ````
        // line closes a ``` ``` ``` fence.
        // kramdown ignores 1–3 leading spaces (OPT_SPACE) before the opening
        // fence (GFM); the body is kept VERBATIM (not de-indented). Under
        // core, only a column-0 fence is in subset.
        let fence_indent = line.len() - line.trim_start_matches(' ').len();
        let fline = if (fence_indent == 0 || opts.gfm) && fence_indent <= 3 {
            &line[fence_indent..]
        } else {
            line
        };
        let fence_char = match fline.as_bytes().first() {
            Some(&b'`') if opts.gfm => Some(b'`'),
            Some(&b'~') => Some(b'~'),
            _ => None,
        };
        let fence_len = fence_char.map_or(0, |c| run_len(fline.as_bytes(), 0, c));
        if let Some(fence_char) = fence_char.filter(|_| fence_len >= 3) {
            let info = fline[fence_len..].trim();
            if info.contains('`') || info.contains('{') {
                return Err(declined("fence-info"));
            }
            let lang = if info.is_empty() {
                None
            } else {
                // The info-string's first word is a sub-slice of the line.
                Some(Cow::Borrowed(info.split_whitespace().next().unwrap_or("")))
            };
            i += 1;
            // Track the first/last CONTENT line so the body — every
            // content line plus its trailing `\n` — can be borrowed as
            // one contiguous `src` slice. Consecutive lines came from
            // `split_lines(src)` separated by exactly one `\n`, and a
            // closing fence always follows the last content line (else
            // we decline), so the `\n` after the last content line is
            // present in `src`. We scan with scalars (no per-fence
            // `Vec<&str>`); the rare non-contiguous case re-walks the
            // content-line range `lines[body_start..body_end]`.
            let body_start = i;
            let mut first_body: Option<&'a str> = None;
            let mut prev_body: Option<&'a str> = None;
            let mut last_body: &'a str = "";
            let mut contiguous = true;
            let mut closed = false;
            while i < lines.len() {
                let l = lines[i];
                // Closing fence: a run of the same char, at least as long
                // as the opener, and nothing else (info strings are not
                // allowed on a closing fence). Under GFM the close may carry
                // 1–3 leading spaces (OPT_SPACE), mirroring the opener.
                let cl = if opts.gfm {
                    let b = l.trim_start_matches(' ');
                    if l.len() - b.len() <= 3 { b } else { l }
                } else {
                    l
                };
                let t = trim_end_ws(cl);
                if t.len() >= fence_len && t.bytes().all(|b| b == fence_char) {
                    closed = true;
                    break;
                }
                if let Some(prev) = prev_body
                    && offset_in(src, l) != offset_in(src, prev) + prev.len() + 1
                {
                    // De-prefixed lines (fence inside a blockquote / list):
                    // not contiguous in `src`, so we must join an owned body.
                    contiguous = false;
                }
                if first_body.is_none() {
                    first_body = Some(l);
                }
                prev_body = Some(l);
                last_body = l;
                i += 1;
            }
            let body_end = i; // exclusive: content lines are [body_start, body_end)
            if closed {
                i += 1; // consume the closing fence line
            }
            if !closed {
                return Err(declined("unclosed-fence"));
            }
            let text = match first_body {
                None => Cow::Borrowed(""),
                Some(first) if contiguous => {
                    // body = src[first.start ..= the `\n` after last] —
                    // each content line plus one trailing newline. The
                    // closing fence guarantees that final `\n` exists.
                    let start = offset_in(src, first);
                    let end = offset_in(src, last_body) + last_body.len() + 1;
                    Cow::Borrowed(&src[start..end])
                }
                Some(_) => {
                    // Non-contiguous: join each content line + `\n`.
                    let mut body = String::with_capacity(64);
                    for l in &lines[body_start..body_end] {
                        body.push_str(l);
                        body.push('\n');
                    }
                    Cow::Owned(body)
                }
            };
            emit_block!(BlockKind::Code { lang, text });
            continue;
        }

        // Blockquote: collect `>`-prefixed lines (plus lazy
        // continuations) and recurse.
        if line.starts_with('>') {
            // The de-`>`-prefixed lines are sub-slices of `src`, so the
            // recursive parse borrows straight from `src` (no owned
            // intermediate `String`s, no deep `into_owned` of the
            // subtree). The `inner` Vec holds only the slice pointers;
            // the data stays in `src`, which outlives the whole AST.
            let mut inner: Vec<&'a str> = Vec::new();
            while i < lines.len() {
                let l = lines[i];
                if let Some(rest) = l.strip_prefix('>') {
                    inner.push(rest.strip_prefix(' ').unwrap_or(rest));
                    i += 1;
                } else if !is_blank(l) && !inner.is_empty() {
                    // A block-IAL line (`{:…}`) is NOT a lazy continuation:
                    // it ends the quote and attaches to the `<blockquote>`
                    // itself (kramdown's "note box" idiom `> …\n{:.note}`).
                    // Break so the top-level scan attaches it to the emitted
                    // Quote block — NOT absorb it into the quote body, where
                    // it would mis-attach to the inner paragraph.
                    let t = l.trim_start_matches(' ');
                    if t.starts_with("{:") && !t.starts_with("{::") {
                        break;
                    }
                    // Lazy continuation of the quoted paragraph.
                    inner.push(l);
                    i += 1;
                } else {
                    break;
                }
            }
            // A quote body never starts/ends with Blank markers — drop
            // the leading/trailing `Blank` nodes from the recursed chain
            // by walking past leading Blanks and stopping the chain
            // before trailing ones, rather than mutating a Vec.
            let mut head = parse_blocks(ast, src, &inner, opts)?;
            // Drop leading Blanks.
            while let Some(h) = head
                && matches!(ast.blocks[h as usize].kind, BlockKind::Blank)
            {
                head = ast.blocks[h as usize].next;
            }
            // Cut the chain before any trailing run of Blanks: walk to the
            // last non-Blank node and clear its `next`.
            if let Some(h) = head {
                let mut cur = h;
                let mut last_keep = h;
                loop {
                    if !matches!(ast.blocks[cur as usize].kind, BlockKind::Blank) {
                        last_keep = cur;
                    }
                    match ast.blocks[cur as usize].next {
                        Some(n) => cur = n,
                        None => break,
                    }
                }
                ast.blocks[last_keep as usize].next = None;
            }
            emit_block!(BlockKind::Quote(head));
            continue;
        }

        // Lists (tight only — blank lines inside / loose shapes
        // decline). Nesting via marker-width indentation is
        // supported for UNORDERED parents (`- a` over `  - b`, the
        // form real posts use): the 2-space-stripped tail of an
        // item parses as continuation text plus at most one child
        // list. Ordered parents keep the conservative decline
        // (their content column is digits+2, not a fixed strip
        // width).
        if let Some(ordered) = list_marker(line) {
            let (items, loose) = parse_list_items(ast, lines, &mut i, ordered)?;
            emit_block!(BlockKind::List {
                ordered,
                loose,
                items
            });
            continue;
        }

        // A list behind a 1–3-space OPT_SPACE base: kramdown ignores the
        // small indent. De-indent the run by the base column (the stripped
        // suffixes still borrow `src`) and parse it as a column-0 list,
        // then skip the lines it consumed.
        let base = line.len() - line.trim_start_matches(' ').len();
        if (1..=3).contains(&base)
            && let Some(ordered) = list_marker(&line[base..])
        {
            let pad = &"   "[..base];
            let deindented: Vec<&str> = lines[i..]
                .iter()
                .map(|l| l.strip_prefix(pad).unwrap_or(l))
                .collect();
            let mut k = 0;
            let (items, loose) = parse_list_items(ast, &deindented, &mut k, ordered)?;
            // The blanket base de-indent also strips a lazy continuation's
            // OWN leading whitespace, but kramdown keeps a lazy line (indent
            // below the item's content column) verbatim. Where a consumed
            // non-marker line had leading space that the de-indent erased,
            // our parse would drop it — decline rather than emit a space-off
            // line. (Indented continuations keep a residual space; markers
            // are structural; both are fine.)
            if (0..k).any(|n| {
                let de = deindented[n];
                !is_blank(de)
                    && list_marker(de).is_none()
                    && lines[i + n].starts_with(' ')
                    && !de.starts_with(' ')
            }) {
                return Err(declined("opt-space-list-continuation"));
            }
            emit_block!(BlockKind::List {
                ordered,
                loose,
                items
            });
            i += k;
            continue;
        }

        // kramdown recognizes most block openers behind 1–3 leading
        // spaces (OPT_SPACE); our dispatcher only sees them at column
        // 0, so an indented opener must decline, not become a paragraph.
        if opt_space_opener(line, opts) {
            return Err(declined("opt-space-block"));
        }

        // Paragraph: gather lines. What ends a paragraph differs by
        // flavor: core kramdown's PARAGRAPH_END is only blank lines
        // (plus IAL/EOB/HTML/deflist starts, all declined); GFM's
        // `paragraph_end` quirk (Jekyll's default) adds LIST_START,
        // ATX_HEADER_START, BLOCKQUOTE_START and FENCED_CODEBLOCK_START
        // — but NOT horizontal rules. Opener-looking lines that don't
        // end the paragraph are literal paragraph text in kramdown.
        // Lines are kept VERBATIM (kramdown preserves interior trailing
        // spaces); only the first line loses its OPT_SPACE indent and
        // the final line is right-stripped.
        // The joined paragraph text is USUALLY a contiguous `src` slice:
        // the first line stripped of its OPT_SPACE indent (a sub-slice),
        // the interior lines verbatim, the `\n` separators that
        // `split_lines` consumed (still present in `src` between the line
        // slices), and the final line right-stripped. When the lines DO
        // abut in `src` (the top-level case) we borrow one slice and
        // never build a `String`. When they DON'T — blockquote / list
        // bodies hand us de-prefixed slices with `> `/indent gaps between
        // them — we fall back to a joined owned `String`.
        let para_start = i;
        let mut first_line: Option<&'a str> = None;
        let mut prev_line: &'a str = "";
        let mut last_line: &'a str = "";
        // True while every line so far abuts the previous one in `src`
        // with exactly one `\n` between — the borrow precondition.
        let mut contiguous = true;
        let mut first = true;
        while i < lines.len() {
            let l = lines[i];
            if is_blank(l) {
                break;
            }
            if opts.gfm
                && !first
                && (l.starts_with('#')
                    || l.starts_with('>')
                    || list_marker(l).is_some()
                    || l.starts_with("```")
                    || l.starts_with("~~~")
                    || crate::html_block::starts_html_block(l))
            {
                break;
            }
            // A block IAL line ends the paragraph; the main loop then
            // attaches its attributes to this paragraph.
            if !first {
                let lt = l.trim_start_matches([' ', '\t']);
                if lt.starts_with("{:")
                    && !lt.starts_with("{::")
                    && l.len() - l.trim_start_matches(' ').len() <= 3
                {
                    break;
                }
            }
            // A swallowed opener-looking line renders as literal text in
            // kramdown; our spans handle `#`/`>` fine, but hr runs would
            // mis-render (`***` → emphasis decline already; `___`/`---`
            // runs likewise) — anything else opener-shaped is rare
            // enough to decline rather than risk divergence.
            if !first && !opts.gfm && (l.starts_with('>') || list_marker(l).is_some()) {
                return Err(declined("core-paragraph-swallow"));
            }
            if !first && opt_space_opener(l, opts) {
                // GFM's paragraph_end includes (OPT_SPACE-indented) list,
                // blockquote, and fence starts — they interrupt the
                // paragraph, so end it here and let the block loop parse the
                // opener. Core's paragraph_end is blank-only, so there the
                // opener is out of subset (declines).
                if opts.gfm {
                    break;
                }
                return Err(declined("opt-space-block"));
            }
            // Setext underlines would silently turn this paragraph into
            // a heading — out of subset.
            if i + 1 < lines.len() {
                let next = lines[i + 1];
                let t = trim_end_ws(next).as_bytes();
                if !t.is_empty() && (t.iter().all(|&b| b == b'=') || t.iter().all(|&b| b == b'-')) {
                    return Err(declined("setext-heading"));
                }
            }
            decline_block_scan(l)?;
            if first {
                // First line: drop OPT_SPACE indent (a sub-slice move).
                first_line = Some(l.trim_start_matches(' '));
            } else {
                // Interior line endings carry hard-break semantics; the
                // just-completed line (`prev_line`) is now interior.
                decline_eol(prev_line)?;
                // Contiguity: this line must abut the previous one in
                // `src` with exactly one `\n` between (true at top level;
                // false once `>`/indent prefixes were stripped).
                if offset_in(src, l) != offset_in(src, prev_line) + prev_line.len() + 1 {
                    contiguous = false;
                }
            }
            prev_line = l;
            last_line = l;
            first = false;
            i += 1;
        }
        let first_line = first_line.expect("paragraph has at least one line");
        // Final paragraph line: kramdown right-strips it (trailing spaces
        // there do NOT produce a hard break).
        let last_stripped = last_line.trim_end_matches([' ', '\t']);
        if contiguous {
            // Reconstruct the contiguous span from the (indent-stripped)
            // first line through the (right-stripped) last line — all
            // verbatim `src`, so the spans borrow directly.
            let start = offset_in(src, first_line);
            let end = offset_in(src, last_stripped) + last_stripped.len();
            let spans = parse_spans(ast, &src[start..end])?;
            emit_block!(BlockKind::Para(spans));
        } else {
            // Gaps between de-prefixed lines (blockquote / list bodies):
            // join `lines[para_start..i]` into an owned `String`
            // (kramdown's verbatim line-join with a single `\n`) — first
            // line indent-stripped, last line right-stripped — parse over
            // it, then push the spans into the arena with their text
            // deep-owned so they don't borrow the temp.
            let para = &lines[para_start..i];
            let last = para.len() - 1; // >= 1: a 1-line para is contiguous
            let mut joined = String::new();
            for (k, &l) in para.iter().enumerate() {
                let segment = if k == 0 {
                    first_line // indent-stripped first line
                } else if k == last {
                    last_stripped // right-stripped final line
                } else {
                    l // interior line, verbatim
                };
                joined.push_str(segment);
                if k != last {
                    joined.push('\n');
                }
            }
            let spans = parse_spans_owned(ast, &joined)?;
            emit_block!(BlockKind::Para(spans));
        }
    }
    Ok(chain.first())
}

/// Parse `joined` (a temporary `String` that does NOT outlive the AST)
/// into the main arena, deep-owning every text `Cow` so no span borrows
/// the temporary. Used for the rare owned-text paragraph fallback
/// (blockquote / list bodies whose de-prefixed source lines don't abut
/// contiguously). Parses into a scratch [`Ast`], then copies the chain
/// in, remapping indices and `into_owned`-ing the Cows.
fn parse_spans_owned<'a>(ast: &mut Ast<'a>, joined: &str) -> Result<Option<u32>, Error> {
    let mut scratch = Ast::new();
    let head = parse_spans(&mut scratch, joined)?;
    Ok(copy_spans_owned(ast, &scratch, head))
}

/// Copy the span chain starting at `head` from `scratch` into `dst`,
/// making every text `Cow` owned (`'static`) so it no longer borrows
/// `scratch`'s backing text. Returns the head index in `dst`.
fn copy_spans_owned<'a>(dst: &mut Ast<'a>, scratch: &Ast<'_>, head: Option<u32>) -> Option<u32> {
    let mut chain = Chain::new();
    let mut cur = head;
    while let Some(idx) = cur {
        let node = &scratch.spans[idx as usize];
        let kind = match &node.kind {
            SpanKind::Text(t) => SpanKind::Text(Cow::Owned(t.clone().into_owned())),
            SpanKind::Raw(t) => SpanKind::Raw(Cow::Owned(t.clone().into_owned())),
            SpanKind::Code(t) => SpanKind::Code(Cow::Owned(t.clone().into_owned())),
            SpanKind::Em(inner) => SpanKind::Em(copy_spans_owned(dst, scratch, *inner)),
            SpanKind::Strong(inner) => SpanKind::Strong(copy_spans_owned(dst, scratch, *inner)),
            SpanKind::Del(inner) => SpanKind::Del(copy_spans_owned(dst, scratch, *inner)),
            SpanKind::Link { spans, href, title } => SpanKind::Link {
                spans: copy_spans_owned(dst, scratch, *spans),
                href: Cow::Owned(href.clone().into_owned()),
                title: title
                    .as_ref()
                    .map(|t| Cow::Owned(t.clone().into_owned())),
            },
            SpanKind::Image { src, alt, title } => SpanKind::Image {
                src: Cow::Owned(src.clone().into_owned()),
                alt: Cow::Owned(alt.clone().into_owned()),
                title: title
                    .as_ref()
                    .map(|t| Cow::Owned(t.clone().into_owned())),
            },
        };
        let new_idx = dst.push_span(kind);
        chain.link(new_idx, |p, n| dst.spans[p as usize].next = Some(n));
        cur = node.next;
    }
    chain.first()
}

/// Constructs we recognize well enough to refuse: kramdown features
/// outside the subset whose silent mis-parse would corrupt output.
/// Split a table row into trimmed cell slices, honouring `\|` escapes and
/// backtick code spans (a `|` inside `` `…` `` is literal, not a cell
/// boundary). Returns `None` when the line has no top-level pipe (not a
/// table row) or has an unbalanced code span (out of the supported subset).
fn split_table_cells(line: &str) -> Option<Vec<&str>> {
    let s = line.trim_matches([' ', '\t']);
    let b = s.as_bytes();
    let mut cells = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut esc = false;
    let mut had_pipe = false;
    while i < b.len() {
        if esc {
            esc = false;
            i += 1;
            continue;
        }
        match b[i] {
            b'\\' => {
                esc = true;
                i += 1;
            }
            b'`' => {
                // Skip a balanced backtick code span (matching run lengths).
                let run = run_len(b, i, b'`');
                let mut j = i + run;
                let mut closed = false;
                while j < b.len() {
                    if b[j] == b'`' {
                        let r2 = run_len(b, j, b'`');
                        if r2 == run {
                            j += run;
                            closed = true;
                            break;
                        }
                        j += r2;
                    } else {
                        j += 1;
                    }
                }
                if !closed {
                    return None;
                }
                i = j;
            }
            b'|' => {
                had_pipe = true;
                cells.push(s[start..i].trim_matches([' ', '\t']));
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    cells.push(s[start..].trim_matches([' ', '\t']));
    if !had_pipe {
        return None;
    }
    // Drop the empty cell created by an optional leading / trailing bar.
    if cells.len() > 1 && cells[0].is_empty() {
        cells.remove(0);
    }
    if cells.len() > 1 && cells.last().is_some_and(|c| c.is_empty()) {
        cells.pop();
    }
    Some(cells)
}

/// Is `line` a table separator line (`---`, `:--`, `--:`, `:-:`, optional
/// `|`)? Must hold at least one `-` and only `-:| \t`.
fn is_table_sep_line(line: &str) -> bool {
    let t = line.trim_matches([' ', '\t']);
    if t.is_empty() {
        return false;
    }
    let mut dash = false;
    for &b in t.as_bytes() {
        match b {
            b'-' => dash = true,
            b':' | b'|' | b' ' | b'\t' => {}
            _ => return false,
        }
    }
    dash
}

/// Per-column alignment from a separator line's cells.
fn sep_align_cells(line: &str) -> Vec<Align> {
    let s = line.trim_matches([' ', '\t']);
    let s = s.strip_prefix('|').unwrap_or(s);
    let s = s.strip_suffix('|').unwrap_or(s);
    s.split('|')
        .map(|c| {
            let c = c.trim_matches([' ', '\t']);
            match (c.starts_with(':'), c.ends_with(':')) {
                (true, true) => Align::Center,
                (true, false) => Align::Left,
                (false, true) => Align::Right,
                (false, false) => Align::None,
            }
        })
        .collect()
}

/// Parse one table row into cell span-heads. `Ok(None)` ⇒ the cell count
/// differs from `ncols` (ragged) or it isn't a row ⇒ the table declines.
fn table_row_spans<'a>(
    ast: &mut Ast<'a>,
    line: &'a str,
    ncols: usize,
) -> Result<Option<Vec<Option<u32>>>, Error> {
    let Some(cells) = split_table_cells(line) else {
        return Ok(None);
    };
    if cells.len() != ncols {
        return Ok(None);
    }
    let mut row = Vec::with_capacity(ncols);
    for cell in cells {
        row.push(parse_spans(ast, cell)?);
    }
    Ok(Some(row))
}

/// Try to parse a kramdown/GFM pipe table starting at `lines[start]`.
/// `Ok(Some((table, consumed)))` for a table in the common shape;
/// `Ok(None)` when it isn't one — heading/list/quote/fence take
/// precedence, and any pipe-shaped run that isn't a clean table falls
/// through to the normal pipe-decline (so output stays right-or-declined).
fn try_parse_table<'a>(
    ast: &mut Ast<'a>,
    lines: &[&'a str],
    start: usize,
    opts: &Options,
) -> Result<Option<(BlockKind<'a>, usize)>, Error> {
    let first = lines[start];
    if first.as_bytes().starts_with(b"    ") || first.as_bytes().first() == Some(&b'\t') {
        return Ok(None);
    }
    let f = trim_start_ws(first);
    if f.starts_with('#')
        || f.starts_with('>')
        || list_marker(first).is_some()
        || f.starts_with("```")
        || f.starts_with("~~~")
        || is_hr(first)
        || is_table_sep_line(first)
    {
        return Ok(None);
    }
    // The first line must itself be a table row.
    if split_table_cells(first).is_none() {
        return Ok(None);
    }
    // Collect the contiguous run; every line must be a row or separator,
    // else the whole block is a paragraph (kramdown), not a table.
    let mut end = start;
    while end < lines.len() {
        let l = lines[end];
        if is_blank(l) {
            break;
        }
        if end > start
            && opts.gfm
            && (l.starts_with('#')
                || l.starts_with('>')
                || list_marker(l).is_some()
                || l.starts_with("```")
                || l.starts_with("~~~"))
        {
            break;
        }
        if trim_start_ws(l).starts_with('+') {
            return Ok(None); // multi-body separator — out of subset
        }
        let is_sep = is_table_sep_line(l);
        // A non-separator line with no top-level pipe isn't a table row, so
        // the block is a paragraph, not a table. (A `<` in a cell is fine —
        // the span parser renders `<=>` literally and declines only real
        // inline HTML / autolinks, keeping output right-or-declined.)
        if !is_sep && split_table_cells(l).is_none() {
            return Ok(None);
        }
        end += 1;
    }
    let run = &lines[start..end];
    // At most one separator, and only as the 2nd line (the header rule).
    let mut sep_idx = None;
    for (k, &l) in run.iter().enumerate() {
        if is_table_sep_line(l) {
            if sep_idx.is_some() {
                return Ok(None);
            }
            sep_idx = Some(k);
        }
    }
    let (header_line, align_line, body): (Option<&str>, Option<&str>, &[&str]) = match sep_idx {
        None => (None, None, run),
        Some(1) => (Some(run[0]), Some(run[1]), &run[2..]),
        Some(_) => return Ok(None),
    };
    if body.is_empty() {
        return Ok(None); // header + separator, no body row ⇒ not a table
    }
    let ncols = match split_table_cells(header_line.unwrap_or(body[0])) {
        Some(c) if !c.is_empty() => c.len(),
        _ => return Ok(None),
    };
    let mut aligns = vec![Align::None; ncols];
    if let Some(al) = align_line {
        for (c, a) in sep_align_cells(al).into_iter().take(ncols).enumerate() {
            aligns[c] = a;
        }
    }
    let header = match header_line {
        Some(h) => match table_row_spans(ast, h, ncols)? {
            Some(r) => Some(r),
            None => return Ok(None),
        },
        None => None,
    };
    let mut rows = Vec::with_capacity(body.len());
    for &l in body {
        match table_row_spans(ast, l, ncols)? {
            Some(r) => rows.push(r),
            None => return Ok(None),
        }
    }
    Ok(Some((
        BlockKind::Table {
            aligns,
            header,
            body: rows,
        },
        end - start,
    )))
}

/// Does `t` hold an unescaped `|` that is NOT inside a balanced backtick
/// code span? Such a pipe is kramdown's table trigger; a `|` inside
/// `` `…` `` is literal. Byte-scanned (`\``|`/backtick are ASCII; a
/// multibyte char's bytes are all ≥ 0x80, so they just advance). An
/// unbalanced code span offers no protection — pipes around it still count.
fn has_table_pipe(t: &str) -> bool {
    let b = t.as_bytes();
    let mut i = 0;
    let mut esc = false;
    while i < b.len() {
        if esc {
            esc = false;
            i += 1;
            continue;
        }
        match b[i] {
            b'\\' => {
                esc = true;
                i += 1;
            }
            b'`' => {
                let run = run_len(b, i, b'`');
                let mut j = i + run;
                let mut closed = false;
                while j < b.len() {
                    if b[j] == b'`' {
                        let r2 = run_len(b, j, b'`');
                        if r2 == run {
                            j += run;
                            closed = true;
                            break;
                        }
                        j += r2;
                    } else {
                        j += 1;
                    }
                }
                i = if closed { j } else { i + run };
            }
            b'|' => return true,
            _ => i += 1,
        }
    }
    false
}

/// kramdown's ATX header-id shorthand: a trailing ` {#id}` (whitespace
/// before `{#`, non-empty preceding text, id of `[A-Za-z0-9_-]`) sets the
/// heading's id and is stripped from the text. Returns `(text_without_id,
/// id)`. `None` if the text doesn't end in that exact shape — `{:.cls}` is
/// literal (not an IAL), `word{#id}` (glued) is literal, and a bare
/// `{#id}` with no preceding text is left for the normal auto-id path
/// (which already slugs it to `id`).
fn extract_header_id(text: &str) -> Option<(&str, &str)> {
    let t = trim_end_ws(text);
    let inner = t.strip_suffix('}')?;
    let open = inner.rfind("{#")?;
    let id = &inner[open + 2..];
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    let before = &inner[..open];
    let before_trimmed = trim_end_ws(before);
    // Require separating whitespace before `{#` and non-empty preceding text.
    if before_trimmed.is_empty() || before.len() == before_trimmed.len() {
        return None;
    }
    Some((before_trimmed, id))
}

/// Whether `lines[idx]` begins a block that a LEADING IAL (`{:.x}` on the
/// line before it) can attach to: a paragraph, heading, blockquote, or
/// list. Returns false for a blank, another IAL, a fence, indented code, an
/// HR, an HTML block, or a table — those either orphan the IAL or render the
/// attribute somewhere we don't model, so the caller declines instead.
fn line_starts_attachable_block(lines: &[&str], idx: usize, opts: &Options) -> bool {
    let Some(&l) = lines.get(idx) else {
        return false;
    };
    if is_blank(l)
        || l.as_bytes().starts_with(b"    ")
        || l.as_bytes().first() == Some(&b'\t')
    {
        return false;
    }
    let t = trim_start_ws(l);
    if t.starts_with("{:")
        || t.starts_with("```")
        || t.starts_with("~~~")
        || is_hr(l)
        || crate::html_block::starts_html_block(l)
        || (has_table_pipe(t) && block_all_pipe(lines, idx, opts))
    {
        return false;
    }
    // Heading / blockquote / list marker / plain paragraph — all attachable.
    true
}

/// Whether EVERY line of the block starting at `start` (until a blank line
/// or a GFM block boundary) carries a table pipe — kramdown's condition for
/// turning the whole block into a table. A block with any pipe-less line is
/// a plain paragraph instead (pipes literal).
fn block_all_pipe(lines: &[&str], start: usize, opts: &Options) -> bool {
    let mut e = start;
    while e < lines.len() {
        let l = lines[e];
        if is_blank(l) {
            break;
        }
        if e > start
            && opts.gfm
            && (l.starts_with('#')
                || l.starts_with('>')
                || list_marker(l).is_some()
                || l.starts_with("```")
                || l.starts_with("~~~")
                || crate::html_block::starts_html_block(l))
        {
            break;
        }
        if !has_table_pipe(trim_start_ws(l)) {
            return false;
        }
        e += 1;
    }
    true
}

fn decline_block_scan(line: &str) -> Result<(), Error> {
    if line.as_bytes().starts_with(b"    ") || line.as_bytes().first() == Some(&b'\t') {
        return Err(declined("indented-code"));
    }
    let t = trim_start_ws(line);
    // NOTE: the table-trigger pipe check is NOT here — kramdown only makes a
    // table when EVERY line of the block is a row, so a lone pipe line is a
    // paragraph. Callers that need the decline (list-item content, where
    // kramdown tables a pipe inside the `<li>`) check `has_table_pipe`
    // explicitly; the block loop routes a non-table pipe block to the
    // paragraph path instead.
    if t.starts_with("{:") || t.starts_with("{::") {
        return Err(declined("ald-ial-extension"));
    }
    if t.starts_with("[^") {
        return Err(declined("footnote"));
    }
    if t.starts_with("*[") {
        return Err(declined("abbreviation"));
    }
    if t.starts_with("$$") {
        return Err(declined("math"));
    }
    if t == "^" {
        return Err(declined("eob-marker"));
    }
    if t.starts_with(": ") || t == ":" {
        return Err(declined("definition-list"));
    }
    // Block-level link reference definitions are lifted out in a pre-pass
    // (see collect_link_defs); a `[id]: url`-looking line that reaches here
    // is mid-paragraph and stays literal text, exactly as in kramdown.
    // Raw HTML blocks (a line opening with a tag).
    let bytes = t.as_bytes();
    if bytes.first() == Some(&b'<')
        && bytes
            .get(1)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'/' || *c == b'!' || *c == b'?')
    {
        return Err(declined("html-block"));
    }
    Ok(())
}

/// Block openers kramdown accepts behind 1–3 leading spaces (OPT_SPACE).
/// A *standalone* opt-space fence is parsed directly by the fence handler;
/// this predicate governs the remaining mid-paragraph / list-continuation
/// contexts, where the indented opener still declines (conservative-safe).
fn opt_space_opener(line: &str, opts: &Options) -> bool {
    let n = line.len() - line.trim_start_matches(' ').len();
    if !(1..=3).contains(&n) {
        return false;
    }
    let s = &line[n..];
    // An indented `#` is NOT a heading in kramdown (GFM or core) — it is
    // ordinary paragraph text, kept verbatim — so it must NOT decline here;
    // only the genuinely block-opening indented forms do.
    s.starts_with('>')
        || list_marker(s).is_some()
        || (opts.gfm && s.starts_with("```"))
        || s.starts_with("~~~")
}

/// kramdown hard-break semantics live in interior paragraph line
/// endings: 2+ trailing spaces (or a trailing backslash) emit
/// `<br />`. Out of subset — decline rather than silently drop the
/// break. Called with the just-completed interior line (the one whose
/// line ending is about to become a `\n` join).
fn decline_eol(last: &str) -> Result<(), Error> {
    // 2+ trailing spaces and a SINGLE trailing `\` are hard line breaks, now
    // rendered as `<br />` by the span parser — no longer declined here.
    let stripped = last.trim_end_matches(' ');
    // ≥2 trailing backslashes before the join (an escaped backslash adjacent
    // to the line-break backslash) is a kramdown corner — decline.
    if stripped.len() - stripped.trim_end_matches('\\').len() >= 2 {
        return Err(declined("eol-backslash"));
    }
    // A trailing TAB is kramdown's own edge (not `(  |\\)`); keep declining.
    if stripped.ends_with('\t') {
        return Err(declined("eol-tab"));
    }
    Ok(())
}

fn is_hr(line: &str) -> bool {
    // An HR is one marker char (`-`/`*`/`_`) repeated >=3, plus spaces/
    // tabs — so the first non-space char fixes the only possible marker.
    // For prose (first char a letter) this bails on byte one, instead of
    // the old three full `chars()` scans (one per candidate marker).
    let t = trim_ws(line).as_bytes();
    let marker = match t.first() {
        Some(&c @ (b'-' | b'*' | b'_')) => c,
        _ => return false,
    };
    let mut count = 0usize;
    for &b in t {
        if b == marker {
            count += 1;
        } else if b != b' ' && b != b'\t' {
            return false;
        }
    }
    count >= 3
}

/// `Some(ordered?)` when the line opens a list item.
/// Collect the items of one (tight) list level starting at
/// `lines[*i]`. Shares the old inline loop's decline rules; the
/// marker-indented tail of an item (stripped by exactly the
/// unordered content column, 2) parses as lazy-continuation text
/// followed by at most one nested child list — which recurses
/// through this same fn, so deeper nesting works and a deeper
/// continuation line attaches to the DEEPEST open item
/// (kramdown's behaviour, probed: `- a` / `  - b` / `    cont`
/// joins `cont` onto b).
fn parse_list_items<'a>(
    ast: &mut Ast<'a>,
    lines: &[&'a str],
    i: &mut usize,
    ordered: bool,
) -> Result<(Option<u32>, bool), Error> {
    // The items of THIS level, sibling-linked.
    let mut items = Chain::new();
    // The currently-open item: its index in the arena and a span Chain so
    // lazy continuations can append to its span run after it was pushed.
    // `None` until the first item opens.
    let mut cur_item: Option<u32> = None;
    let mut cur_spans = Chain::new();
    let mut cur_has_child = false;
    // `loose`: some adjacent item pair is blank-separated. `tight_adjacent`:
    // some pair abuts with no blank. A list mixing both renders per-item in
    // kramdown (out of subset) — we accept only uniformly tight or uniformly
    // loose lists and decline the mix. `via_blank` flags that the next item
    // was reached across a blank line.
    let mut loose = false;
    let mut tight_adjacent = false;
    let mut via_blank = false;
    while *i < lines.len() {
        let l = lines[*i];
        if is_blank(l) {
            // A blank line between two items makes the whole list LOOSE
            // (each item's content wraps in `<p>`); otherwise the list
            // ends here.
            let mut j = *i;
            while j < lines.len() && is_blank(lines[j]) {
                j += 1;
            }
            // `* * *` / `- - -` look like a marker but are a horizontal
            // rule that ends the list (kramdown), not a loose continuation.
            if j < lines.len() && list_marker(lines[j]) == Some(ordered) && !is_hr(lines[j]) {
                // A loose list whose items carry a nested child (or
                // multiple blocks) is out of the v1 subset.
                if cur_has_child {
                    return Err(declined("loose-list-child"));
                }
                loose = true;
                via_blank = true;
                *i = j;
                continue;
            }
            // A blank followed by an indented line is a second block of the
            // current item (a multi-paragraph / nested-block item) — out of
            // subset; decline rather than mis-parse it as a separate block.
            if j < lines.len() && lines[j].starts_with(' ') {
                return Err(declined("list-item-multiblock"));
            }
            break;
        }
        if list_marker(l) == Some(ordered) {
            // A new item abutting the previous one (no blank between) is a
            // tight adjacency; one reached across a blank was already
            // recorded as loose.
            if cur_item.is_some() && !via_blank {
                tight_adjacent = true;
            }
            via_blank = false;
            let content = strip_marker(l, ordered);
            // Item content is block-level in kramdown — tables,
            // EOB markers, IALs etc. inside an item are out of
            // subset, same as at the top level. A `|` inside an item makes
            // kramdown render a `<table>` inside the `<li>` (out of subset).
            if has_table_pipe(trim_start_ws(content)) {
                return Err(declined("table"));
            }
            decline_block_scan(content)?;
            // Trailing whitespace carries hard-break semantics.
            if trim_end_ws(content) != content {
                return Err(declined("list-trailing-ws"));
            }
            // Flush the previous item's span tail back into its node
            // before opening the next item.
            if let Some(prev) = cur_item {
                ast.items[prev as usize].spans = cur_spans.first();
            }
            let mut spans = Chain::new();
            let parsed = parse_spans(ast, content)?;
            chain_extend_spans(ast, &mut spans, parsed);
            let item_idx = ast.push_item(ItemNode {
                spans: spans.first(),
                child: None,
                next: None,
                _marker: std::marker::PhantomData,
            });
            items.link(item_idx, |p, n| ast.items[p as usize].next = Some(n));
            cur_item = Some(item_idx);
            cur_spans = spans;
            cur_has_child = false;
            *i += 1;
            // Marker-indented tail block: lines indented to at least the
            // item's content column attach to THIS item, stripped by that
            // column. The column is the marker width — 2 for `- `/`* `/
            // `+ `, digits+2 for `1. ` — so ordered lists now carry an
            // indented continuation / nested child, not just unordered.
            // Tabs decline; an indent of 2..col (only reachable for ordered
            // markers wider than 2) is kramdown's space-keeping lazy form,
            // out of our clean subset.
            let content_col = l.len() - content.len();
            let mut tail: Vec<&str> = Vec::new();
            while *i < lines.len() && !is_blank(lines[*i]) {
                let tl = lines[*i];
                if tl.starts_with('\t') {
                    return Err(declined("list-tab-indent"));
                }
                let lead = tl.len() - tl.trim_start_matches(' ').len();
                if lead < 2 {
                    break;
                }
                if lead < content_col {
                    return Err(declined("list-continuation"));
                }
                tail.push(&tl[content_col..]);
                *i += 1;
            }
            if !tail.is_empty() {
                let mut j = 0usize;
                // Leading non-marker lines: lazy continuations of
                // this item (kramdown joins them verbatim with a
                // newline, indentation stripped).
                while j < tail.len() && list_marker(tail[j]).is_none() {
                    let cont = tail[j];
                    if has_table_pipe(trim_start_ws(cont)) {
                        return Err(declined("table"));
                    }
                    decline_block_scan(cont)?;
                    if trim_end_ws(cont) != cont || cont.starts_with(' ') || cont.starts_with('\t') {
                        return Err(declined("list-continuation-ws"));
                    }
                    let parsed = parse_spans(ast, cont)?;
                    // A bare emphasis/code marker on either side of the join
                    // would pair across the line break under kramdown; our
                    // per-line parse leaves both literal, so decline.
                    if chain_has_bare_marker(ast, parsed)
                        || chain_has_bare_marker(ast, cur_spans.first())
                    {
                        return Err(declined("list-continuation-inline-span"));
                    }
                    let nl = ast.push_span(SpanKind::Text(Cow::Borrowed("\n")));
                    cur_spans.link(nl, |p, n| ast.spans[p as usize].next = Some(n));
                    chain_extend_spans(ast, &mut cur_spans, parsed);
                    j += 1;
                }
                let child = if j < tail.len() {
                    // A nested child inside a loose list is out of subset.
                    if loose {
                        return Err(declined("loose-list-child"));
                    }
                    // Child list: recurse over the rest of the
                    // stripped tail. Deeper-indented lines inside
                    // recurse again; a trailing stripped non-marker
                    // line is the child's own lazy continuation.
                    let child_ordered = list_marker(tail[j])
                        .expect("loop exit condition");
                    let mut k = j;
                    let (child_items, child_loose) =
                        parse_list_items(ast, &tail, &mut k, child_ordered)?;
                    if child_loose {
                        // A loose nested list is out of the v1 subset.
                        return Err(declined("loose-nested-list"));
                    }
                    if k < tail.len() {
                        // Content after the child list inside the
                        // same item (blank-separated etc.) — out of
                        // subset.
                        return Err(declined("list-after-child"));
                    }
                    Some((child_ordered, child_items))
                } else {
                    None
                };
                // Persist the (possibly extended) span tail and child.
                let item = cur_item.expect("just pushed");
                ast.items[item as usize].spans = cur_spans.first();
                ast.items[item as usize].child = child;
                cur_has_child = child.is_some();
            }
        } else if l.starts_with(' ') {
            // Sub-2-space indent (1 space): kramdown treats a
            // 1-space marker as a SAME-level item; conservatively
            // decline the whole family.
            return Err(declined("list-continuation"));
        } else if list_marker(l).is_some() {
            return Err(declined("mixed-list-markers"));
        } else if l.starts_with("{:") && !l.starts_with("{::") {
            // A block-IAL line abutting the list terminates it; the main
            // loop then attaches the IAL to the emitted list block. (We
            // only reach here at column 0 — indented `{:` was caught by the
            // leading-space arm above.)
            break;
        } else {
            // Lazy continuation line appended to the last item.
            if has_table_pipe(trim_start_ws(l)) {
                return Err(declined("table"));
            }
            decline_block_scan(l)?;
            if trim_end_ws(l) != l {
                return Err(declined("list-continuation-ws"));
            }
            match cur_item {
                Some(item) => {
                    if cur_has_child {
                        // Column-0 text after a nested child would
                        // join the PARENT item in kramdown — out of
                        // our emit shape.
                        return Err(declined("list-after-child"));
                    }
                    let parsed = parse_spans(ast, l)?;
                    // See the tail-continuation arm: a bare emphasis/code
                    // marker that would pair across the line break declines.
                    if chain_has_bare_marker(ast, parsed)
                        || chain_has_bare_marker(ast, cur_spans.first())
                    {
                        return Err(declined("list-continuation-inline-span"));
                    }
                    let nl = ast.push_span(SpanKind::Text(Cow::Borrowed("\n")));
                    cur_spans.link(nl, |p, n| ast.spans[p as usize].next = Some(n));
                    chain_extend_spans(ast, &mut cur_spans, parsed);
                    ast.items[item as usize].spans = cur_spans.first();
                    *i += 1;
                }
                None => break,
            }
        }
    }
    // Persist the final open item's span tail.
    if let Some(prev) = cur_item {
        ast.items[prev as usize].spans = cur_spans.first();
    }
    // A list mixing blank-separated and abutting items renders per-item in
    // kramdown — out of subset.
    if loose && tight_adjacent {
        return Err(declined("mixed-loose-tight-list"));
    }
    Ok((items.first(), loose))
}

/// Append an already-built span chain (head index `head`, or `None`) to
/// `chain`, linking its head onto the current tail and advancing the
/// chain's `last` to the appended chain's tail so subsequent links extend
/// past it. Mirrors `Vec<Span>::extend` for the sibling-linked arena.
fn chain_extend_spans(ast: &mut Ast<'_>, chain: &mut Chain, head: Option<u32>) {
    let Some(head) = head else { return };
    chain.link(head, |p, n| ast.spans[p as usize].next = Some(n));
    // Walk to the appended chain's tail and make it the new chain tail.
    let mut tail = head;
    while let Some(n) = ast.spans[tail as usize].next {
        tail = n;
    }
    chain.last = Some(tail);
}

/// Whether the top-level span chain `head` carries an inline delimiter the
/// per-line parse couldn't pair within its own physical line: a bare `*` or
/// backtick in a Text node, or an unbalanced `[`/`]` count (a link whose
/// brackets straddle the line break). In a multi-line list item such a
/// delimiter pairs with one on another line under kramdown's
/// join-then-parse, which our zero-copy per-line parse can't reproduce (the
/// joined text would need an owned buffer outliving the `&'a src` borrow),
/// so the item declines.
///
/// `_` is deliberately excluded — intra-word underscores (`runtime_deps`)
/// are literal in both engines and would over-decline. Balanced brackets
/// (`arr[0]`, an unresolved `[note]`) stay literal in both engines, so only
/// an imbalance — the genuine cross-line-link signal — trips this.
fn chain_has_bare_marker(ast: &Ast<'_>, head: Option<u32>) -> bool {
    let mut cur = head;
    let mut bracket_depth: i32 = 0;
    while let Some(idx) = cur {
        if let SpanKind::Text(t) = &ast.spans[idx as usize].kind {
            for b in t.bytes() {
                match b {
                    b'*' | b'`' => return true,
                    b'[' => bracket_depth += 1,
                    b']' => bracket_depth -= 1,
                    _ => {}
                }
            }
        }
        cur = ast.spans[idx as usize].next;
    }
    bracket_depth != 0
}

fn list_marker(line: &str) -> Option<bool> {
    let b = line.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'*' | b'+' | b'-') && (b[1] == b' ' || b[1] == b'\t') {
        return Some(false);
    }
    let digits = line.bytes().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0
        && b.len() > digits + 1
        && b[digits] == b'.'
        && (b[digits + 1] == b' ' || b[digits + 1] == b'\t')
    {
        return Some(true);
    }
    None
}

fn strip_marker(line: &str, ordered: bool) -> &str {
    if ordered {
        let digits = line.bytes().take_while(|c| c.is_ascii_digit()).count();
        line[digits + 1..].trim_start_matches([' ', '\t'])
    } else {
        line[1..].trim_start_matches([' ', '\t'])
    }
}

// ---- span parsing -------------------------------------------------------

/// Span element kinds — kramdown blocks same-type nesting (an `em`
/// anywhere inside an `em` stays literal) and gates the strong→em
/// retry on the immediate parent.
#[derive(Clone, Copy, PartialEq)]
enum Elem {
    Em,
    Strong,
    Del,
    Link,
}

/// The emphasis close being searched for by a `parse_spans_until`
/// invocation (kramdown's `stop_re` + its acceptance conditions). The
/// delimiter is always `delim_len` (1 or 2) copies of `type_char`, so we
/// carry just those two bytes — no heap-allocated delimiter `String`
/// (one per emphasis attempt; emphasis is common in prose).
struct Stop {
    type_char: u8,
    delim_len: usize,
    elem: Elem,
}

impl Stop {
    /// Whether `s` opens with this stop's delimiter run (`delim_len`
    /// copies of `type_char`). Equivalent to the old
    /// `s.starts_with(stop.delim)`.
    #[inline]
    fn matches_at(&self, s: &str) -> bool {
        let b = s.as_bytes();
        b.len() >= self.delim_len && b[..self.delim_len].iter().all(|&x| x == self.type_char)
    }
}

/// Ruby `/\s/` is ASCII-only — `char::is_whitespace` would also match
/// U+00A0 etc. and silently diverge.
fn ruby_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// Accumulates the running `Text` span of `parse_spans_until`. While the
/// output bytes are byte-identical to the source, it tracks just the
/// `[seg_start, seg_end)` byte range of `text` and emits a borrowed
/// `Cow::Borrowed` slice on `flush` — no allocation. The first time a
/// typography rewrite (`'`→`'`, `--`→`–`, `...`→`…`) or a backslash
/// escape changes a char, it copies the pristine range collected so far
/// into an owned `String` and switches to `push`-ing into it.
///
/// SYNERGY with `next_trigger`: the inline parser's `_` arm scans a run
/// of ordinary bytes with `next_trigger`; that run is a pristine `text`
/// slice, so it's recorded as a borrow extent (`push_verbatim`) rather
/// than copied — the whole point of the zero-copy AST.
struct TextRun<'a> {
    text: &'a str,
    seg_start: usize,
    seg_end: usize,
    owned: Option<String>,
}

impl<'a> TextRun<'a> {
    #[inline]
    fn new(text: &'a str) -> Self {
        TextRun { text, seg_start: 0, seg_end: 0, owned: None }
    }

    #[inline]
    fn is_empty(&self) -> bool {
        match &self.owned {
            Some(s) => s.is_empty(),
            None => self.seg_start == self.seg_end,
        }
    }

    /// Begin a fresh segment at `pos` if the accumulator is currently
    /// empty (i.e. right after a `flush`, before any byte of the next
    /// segment has been recorded). A no-op once a segment is in progress.
    #[inline]
    fn restart_if_empty(&mut self, pos: usize) {
        if self.owned.is_none() && self.seg_start == self.seg_end {
            self.seg_start = pos;
            self.seg_end = pos;
        }
    }

    /// Record verbatim source bytes `text[a..b]` — identical input and
    /// output, so they extend the borrowed segment (or, once owned, get
    /// copied in bulk). `a` must abut the current segment end.
    #[inline]
    fn push_verbatim(&mut self, a: usize, b: usize) {
        self.restart_if_empty(a);
        debug_assert_eq!(a, self.seg_end, "non-contiguous verbatim run");
        if let Some(owned) = &mut self.owned {
            owned.push_str(&self.text[a..b]);
        }
        self.seg_end = b;
    }

    /// Record a single verbatim source byte at `i` (an ASCII trigger
    /// char emitted literally — its guarded arm didn't fire). Same as a
    /// 1-byte verbatim run.
    #[inline]
    fn push_byte(&mut self, i: usize) {
        self.push_verbatim(i, i + 1);
    }

    /// Record a rewritten char `ch` that replaces source `text[a..b]`
    /// (typography / escape). This breaks the borrow: materialize the
    /// pristine prefix once, then push `ch`. `a` must abut the segment.
    #[inline]
    fn push_char(&mut self, ch: char, a: usize, b: usize) {
        self.restart_if_empty(a);
        debug_assert_eq!(a, self.seg_end, "non-contiguous rewrite");
        let owned = self.owned.get_or_insert_with(|| {
            // Copy the verbatim prefix collected so far, then diverge.
            self.text[self.seg_start..self.seg_end].to_owned()
        });
        owned.push(ch);
        self.seg_end = b;
    }

    /// Emit the accumulated `Text` span (if any) into the arena, linking
    /// it onto `chain`, then reset to empty.
    #[inline]
    fn flush(&mut self, ast: &mut Ast<'a>, chain: &mut Chain) {
        match self.owned.take() {
            Some(owned) => {
                if !owned.is_empty() {
                    let idx = ast.push_span(SpanKind::Text(Cow::Owned(owned)));
                    chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                }
            }
            None => {
                if self.seg_start < self.seg_end {
                    let idx = ast.push_span(SpanKind::Text(Cow::Borrowed(
                        &self.text[self.seg_start..self.seg_end],
                    )));
                    chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                }
            }
        }
        // Collapse to empty; the next push restarts at its own position.
        self.seg_start = self.seg_end;
    }
}

pub(crate) fn parse_spans<'a>(ast: &mut Ast<'a>, text: &'a str) -> Result<Option<u32>, Error> {
    let (head, _) = parse_spans_until(ast, text, None, false, false, None)?;
    Ok(head)
}

/// Recursive-descent span parser mirroring kramdown's `parse_spans` +
/// `parse_emphasis`: scans `text`, optionally watching for an emphasis
/// `stop` delimiter. Returns the spans plus `Some(pos)` where the
/// accepted close begins, or `None` if the text ran out (the caller
/// then reverts to literal delimiters, like kramdown's `revert_pos`).
fn parse_spans_until<'a>(
    ast: &mut Ast<'a>,
    text: &'a str,
    stop: Option<&Stop>,
    in_em: bool,
    in_strong: bool,
    parent: Option<Elem>,
) -> Result<(Option<u32>, Option<usize>), Error> {
    // The span run this call produces, sibling-linked in `ast.spans`.
    let mut chain = Chain::new();
    // Borrowing text accumulator (replaces a `String` buf): a run of
    // verbatim bytes is emitted as `Cow::Borrowed`; only a rewrite forces
    // an owned `String`.
    let mut acc = TextRun::new(text);
    let bytes = text.as_bytes();
    let mut i = 0;
    // Last logical character, across span boundaries — smart-quote
    // open/close classification and emphasis-close pre-checks need it
    // (kramdown sees the raw source via pre_match).
    let mut prev: Option<char> = None;
    while i < bytes.len() {
        // Emphasis close? kramdown checks the stop_re before running
        // span parsers, with these acceptance conditions; a rejected
        // candidate falls through to normal parsing (where it may OPEN
        // a nested span of a different type).
        if let Some(stop) = stop
            && stop.matches_at(&text[i..])
        {
            let content_nonempty = chain.first().is_some() || !acc.is_empty();
            let prev_ok = prev.is_some_and(|c| !ruby_space(c));
            // An em close can't sit on a clean strong delimiter
            // (`**` not followed by a third `*`) — that position
            // belongs to a nested strong.
            let em_ok = stop.elem != Elem::Em || run_len(bytes, i, stop.type_char) != 2;
            // `_` closes don't bind into a following word.
            let underscore_ok = stop.type_char != b'_'
                || !text[i + stop.delim_len..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric);
            if content_nonempty && prev_ok && em_ok && underscore_ok {
                acc.flush(ast, &mut chain);
                return Ok((chain.first(), Some(i)));
            }
        }
        let c = bytes[i];
        match c {
            b'\\' if i + 1 < bytes.len() => {
                let next = bytes[i + 1] as char;
                // A backslash immediately before a newline is a hard line
                // break (kramdown), like trailing double-space.
                if next == '\n' {
                    acc.flush(ast, &mut chain);
                    let br = ast.push_span(SpanKind::Raw(Cow::Borrowed("<br />")));
                    chain.link(br, |q, n| ast.spans[q as usize].next = Some(n));
                    prev = Some('\n');
                    i += 1; // consume the `\`; the `\n` stays as joined text
                    continue;
                }
                // kramdown's exact ESCAPED_CHARS set; anything else
                // keeps the backslash literally.
                if "\\.*_+`<>()[]{}#!:|\"'$=-".contains(next) {
                    // The escape drops the `\`, so the output (`next`)
                    // differs from the 2 source bytes — a rewrite.
                    acc.push_char(next, i, i + 2);
                    prev = Some(next);
                    i += 2;
                } else {
                    // Lone `\` kept verbatim.
                    acc.push_byte(i);
                    prev = Some('\\');
                    i += 1;
                }
            }
            b'`' => {
                // Code span with N-backtick delimiter.
                let open = run_len(bytes, i, b'`');
                let delim = &text[i..i + open];
                let rest = &text[i + open..];
                // kramdown: a SINGLE backtick surrounded by whitespace
                // (or start of text) is a literal backtick, not a span.
                if open == 1
                    && prev.is_none_or(char::is_whitespace)
                    && rest.chars().next().is_some_and(char::is_whitespace)
                {
                    acc.push_byte(i); // literal backtick, verbatim
                    prev = Some('`');
                    i += 1;
                    continue;
                }
                // No closing delimiter: kramdown resets and emits the
                // backticks as literal text (verbatim source).
                let Some(close_rel) = rest.find(delim) else {
                    acc.push_verbatim(i, i + open);
                    prev = Some('`');
                    i += open;
                    continue;
                };
                acc.flush(ast, &mut chain);
                // kramdown trims one leading and one trailing space —
                // independently — for multi-backtick delimiters only.
                // `inner` stays a sub-slice of `text`, so it borrows.
                let mut inner = &rest[..close_rel];
                if open > 1 {
                    inner = inner.strip_prefix(' ').unwrap_or(inner);
                    inner = inner.strip_suffix(' ').unwrap_or(inner);
                }
                let idx = ast.push_span(SpanKind::Code(Cow::Borrowed(inner)));
                chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                prev = Some('`');
                i += open + close_rel + open;
            }
            b'*' | b'_' => {
                // kramdown EMPHASIS_START takes at most two delimiter
                // chars; a longer run leaves the rest as content.
                let take = run_len(bytes, i, c).min(2);
                // Intra-word underscore bail:
                // pre_match =~ /[[:alpha:]]-?[[:alpha:]]*_*\z/.
                if c == b'_' && underscore_intraword(&text[..i], prev) {
                    acc.push_verbatim(i, i + take); // verbatim `_`(s)
                    prev = Some('_');
                    i += take;
                    continue;
                }
                let elem = if take == 2 { Elem::Strong } else { Elem::Em };
                let same_type = (elem == Elem::Em && in_em) || (elem == Elem::Strong && in_strong);
                let opens_on_space = text[i + take..].chars().next().is_some_and(ruby_space);
                if same_type || opens_on_space {
                    acc.push_verbatim(i, i + take); // literal delimiter run
                    prev = Some(c as char);
                    i += take;
                    continue;
                }
                let attempt = Stop {
                    type_char: c,
                    delim_len: take,
                    elem,
                };
                // BACKTRACK: record the arena length before the speculative
                // recursion so abandoned inner nodes can be reverted.
                let saved = ast.spans.len();
                let (inner, close) = parse_spans_until(
                    ast,
                    &text[i + take..],
                    Some(&attempt),
                    in_em || elem == Elem::Em,
                    in_strong || elem == Elem::Strong,
                    Some(elem),
                )?;
                if let Some(close) = close {
                    acc.flush(ast, &mut chain);
                    let idx = ast.push_span(if take == 2 {
                        SpanKind::Strong(inner)
                    } else {
                        SpanKind::Em(inner)
                    });
                    chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                    prev = Some(c as char);
                    i += take + close + take;
                    continue;
                }
                // Reverted: drop the speculative inner nodes so they don't
                // leak into the arena or the output.
                ast.spans.truncate(saved);
                // Unclosed strong retries from pos+1 as a single-char
                // em, unless the immediate parent is an em.
                if elem == Elem::Strong && parent != Some(Elem::Em) {
                    let retry = Stop {
                        type_char: c,
                        delim_len: 1,
                        elem: Elem::Em,
                    };
                    let saved = ast.spans.len();
                    let (inner, close) = parse_spans_until(
                        ast,
                        &text[i + 1..],
                        Some(&retry),
                        true,
                        in_strong,
                        Some(Elem::Em),
                    )?;
                    if let Some(close) = close {
                        acc.flush(ast, &mut chain);
                        let idx = ast.push_span(SpanKind::Em(inner));
                        chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                        prev = Some(c as char);
                        i += 1 + close + 1;
                        continue;
                    }
                    // Reverted again: drop the retry's speculative nodes.
                    ast.spans.truncate(saved);
                }
                // No close anywhere: kramdown reverts and emits the
                // delimiter run as literal text (verbatim source).
                acc.push_verbatim(i, i + take);
                prev = Some(c as char);
                i += take;
            }
            b'[' => {
                acc.flush(ast, &mut chain);
                let rest = &text[i..];
                // kramdown forbids nested links: inside a link's text a
                // `[…](…)` stays literal (an image `![…](…)` is still fine,
                // handled by the `!` arm). So only attempt a link when not
                // already inside one.
                if parent != Some(Elem::Link)
                    && let Some((spans, href, title, len)) =
                        parse_link(ast, rest, in_em, in_strong)?
                {
                    let idx = ast.push_span(SpanKind::Link { spans, href, title });
                    chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                    prev = Some(')');
                    i += len;
                } else {
                    // Not an inline link → `[` is literal; resume after it.
                    acc.push_byte(i);
                    prev = Some('[');
                    i += 1;
                }
            }
            b'!' if bytes.get(i + 1) == Some(&b'[') => {
                acc.flush(ast, &mut chain);
                let rest = &text[i..];
                if let Some((src, alt, title, len)) = parse_image(ast, rest)? {
                    let idx = ast.push_span(SpanKind::Image { src, alt, title });
                    chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                    prev = Some(')');
                    i += len;
                } else {
                    // Not an inline image → `!` is literal; resume after it
                    // (the `[` is then handled by the bracket arm).
                    acc.push_byte(i);
                    prev = Some('!');
                    i += 1;
                }
            }
            b'<' => {
                // Autolinks / inline HTML are out of subset; a bare `<`
                // followed by space/punct is plain text.
                let next = bytes.get(i + 1).copied();
                // An inline HTML element is re-serialized to match kramdown:
                // void → ` />`, raw-content (`<code>`/…) → escaped body, and
                // a normal element's content is parsed as markdown. Anything
                // else (block-level inline, autolink, comment) declines.
                if next.is_some_and(|c| c.is_ascii_alphabetic())
                    && let Some((el, len)) = crate::html_block::inline_at(&text[i..])
                {
                    acc.flush(ast, &mut chain);
                    let push_raw = |ast: &mut Ast<'a>, chain: &mut Chain, s: String| {
                        let idx = ast.push_span(SpanKind::Raw(Cow::Owned(s)));
                        chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                    };
                    match el {
                        crate::html_block::Inline::Void(html) => push_raw(ast, &mut chain, html),
                        crate::html_block::Inline::Raw(html) => push_raw(ast, &mut chain, html),
                        crate::html_block::Inline::Markdown { open, content, close } => {
                            push_raw(ast, &mut chain, open);
                            let inner = parse_spans(ast, content)?;
                            chain_extend_spans(ast, &mut chain, inner);
                            push_raw(ast, &mut chain, close);
                        }
                    }
                    prev = Some('>');
                    i += len;
                    continue;
                }
                if next.is_some_and(|c| c.is_ascii_alphabetic() || c == b'/' || c == b'!') {
                    return Err(declined("inline-html-or-autolink"));
                }
                if next == Some(b'<') {
                    // kramdown turns `<<`/`>>` into guillemets and folds an
                    // adjacent space into a non-breaking space (`<< ` → «\u{a0},
                    // ` >>` → \u{a0}»). The trailing-space case needs a
                    // backward rewrite the forward-only text accumulator
                    // can't do, so decline rather than emit a near-miss.
                    return Err(declined("guillemets"));
                }
                acc.push_byte(i); // literal `<`, verbatim
                prev = Some('<');
                i += 1;
            }
            b'>' if bytes.get(i + 1) == Some(&b'>') => {
                return Err(declined("guillemets"));
            }
            b'&' => {
                // Resolve an HTML entity to its character (kramdown's
                // `:as_char` output). A reference that isn't a known
                // entity — unknown name, malformed numeric, no `;` — leaves
                // the `&` literal, and `escape_text` then emits `&amp;…`,
                // exactly as kramdown does for an unrecognized entity.
                if let Some((cp, len)) = parse_entity(&text[i..]) {
                    // `&`, `<`, `>` stay escaped, and kramdown preserves the
                    // SOURCE form: a NAMED `&amp;` re-emits `&amp;` (which a
                    // pushed `&` + escape_text reproduces exactly), but a
                    // NUMERIC `&#38;` re-emits `&#38;`, which escaping a `&`
                    // can't — so decline the numeric form of those three.
                    if matches!(cp, 0x26 | 0x3C | 0x3E) && bytes[i + 1] == b'#' {
                        return Err(declined("numeric-special-entity"));
                    }
                    match char::from_u32(cp) {
                        // The decoded char is pushed as a rewrite — it is NOT
                        // re-scanned, so `&#42;` stays a literal `*`, never
                        // emphasis, matching kramdown.
                        Some(ch) => {
                            acc.push_char(ch, i, i + len);
                            prev = Some(ch);
                            i += len;
                        }
                        // Out-of-range / surrogate code point: kramdown emits
                        // U+FFFD replacements we don't reproduce — decline.
                        None => return Err(declined("invalid-entity-codepoint")),
                    }
                } else {
                    acc.push_byte(i); // literal `&`, verbatim
                    prev = Some('&');
                    i += 1;
                }
            }
            b'~' if bytes.get(i + 1) == Some(&b'~') => {
                // GFM strikethrough `~~text~~`. Like emphasis: opens unless
                // the next char is a space; the close (generic stop check)
                // needs non-empty content not preceded by a space. A run of
                // 3+ tildes inline is a rarer kramdown form — decline.
                if run_len(bytes, i, b'~') != 2 {
                    return Err(declined("strikethrough"));
                }
                let opens_on_space = text[i + 2..].chars().next().is_some_and(ruby_space);
                if parent == Some(Elem::Del) || opens_on_space {
                    acc.push_verbatim(i, i + 2);
                    prev = Some('~');
                    i += 2;
                    continue;
                }
                let attempt = Stop {
                    type_char: b'~',
                    delim_len: 2,
                    elem: Elem::Del,
                };
                let saved = ast.spans.len();
                let (inner, close) = parse_spans_until(
                    ast,
                    &text[i + 2..],
                    Some(&attempt),
                    in_em,
                    in_strong,
                    Some(Elem::Del),
                )?;
                if let Some(close) = close {
                    acc.flush(ast, &mut chain);
                    let idx = ast.push_span(SpanKind::Del(inner));
                    chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                    prev = Some('~');
                    i += 2 + close + 2;
                    continue;
                }
                // No close: kramdown reverts to literal `~~`.
                ast.spans.truncate(saved);
                acc.push_verbatim(i, i + 2);
                prev = Some('~');
                i += 2;
            }
            b'{' if bytes.get(i + 1) == Some(&b':') => {
                // Span IAL: `{:…}` immediately after a span element (no text
                // between) attaches its attributes to it. `{::` extensions,
                // non-adjacent forms, and IALs on code/text decline.
                if bytes.get(i + 2) != Some(&b':')
                    && acc.is_empty()
                    && let Some(last) = chain.last
                    && matches!(
                        ast.spans[last as usize].kind,
                        SpanKind::Link { .. }
                            | SpanKind::Image { .. }
                            | SpanKind::Em(_)
                            | SpanKind::Strong(_)
                    )
                    && let Some(close_rel) = text[i + 2..].find('}')
                    && let Some(attrs) = parse_ial(&text[i + 2..i + 2 + close_rel])
                {
                    // A second IAL abutting the same span (`…{:.a}{:.b}`)
                    // would need kramdown's cross-IAL attribute merge (class
                    // accumulation, id override) we don't model — declining
                    // beats dropping the first or emitting a duplicate attr.
                    if ast.span_ials.contains_key(&last) {
                        return Err(declined("chained-span-ial"));
                    }
                    ast.span_ials.insert(last, attrs);
                    i += 2 + close_rel + 1;
                    continue;
                }
                return Err(declined("ial-or-extension"));
            }
            b'\'' | b'"' => {
                if run_len(bytes, i, c) > 1 {
                    return Err(declined("quote-run"));
                }
                // A smart quote immediately after a code span sits on a
                // boundary kramdown classifies with its intricate SQ_RULES
                // (e.g. `` `x`'d `` opens but `` `x`'s `` closes) — we can't
                // reproduce that reliably, so decline.
                if acc.is_empty()
                    && chain
                        .last
                        .is_some_and(|l| matches!(ast.spans[l as usize].kind, SpanKind::Code(_)))
                {
                    return Err(declined("quote-after-code"));
                }
                let q = typography::smart_quote(prev, c == b'\'', &text[i + 1..])?;
                // Smart quote: the emitted char (U+2018..U+201D) always
                // differs from the ASCII source byte — a rewrite.
                acc.push_char(q, i, i + 1);
                prev = Some(q);
                i += 1;
            }
            b'-' => {
                let dash_run = run_len(bytes, i, c);
                let sym = match dash_run {
                    1 => '-',
                    2 => typography::NDASH,
                    3 => typography::MDASH,
                    _ => return Err(declined("dash-run")),
                };
                if dash_run == 1 {
                    // A lone `-` is itself — keep the borrow.
                    acc.push_byte(i);
                } else {
                    // `--`/`---` collapse to a single en/em dash: rewrite.
                    acc.push_char(sym, i, i + dash_run);
                }
                prev = Some(sym);
                i += dash_run;
            }
            b'.' => {
                let dot_run = run_len(bytes, i, c);
                match dot_run {
                    1 | 2 => {
                        // `.`/`..` are themselves — verbatim source.
                        acc.push_verbatim(i, i + dot_run);
                        prev = Some('.');
                    }
                    3 => {
                        // `...` → `…`: rewrite.
                        acc.push_char(typography::HELLIP, i, i + 3);
                        prev = Some(typography::HELLIP);
                    }
                    _ => return Err(declined("ellipsis-run")),
                }
                i += dot_run;
            }
            _ if is_trigger(c) => {
                // A trigger byte whose guarded arm didn't fire (e.g. `!`
                // not before `[`, `>` not before `>`, a trailing `\`).
                // All triggers are ASCII, so this is the whole char,
                // emitted verbatim.
                acc.push_byte(i);
                prev = Some(c as char);
                i += 1;
            }
            _ => {
                // SYNERGY: scan a run of ordinary bytes with the existing
                // `next_trigger` (scalar TRIGGER table, or NEON byteset
                // under `--features simd`); the run is a pristine slice
                // of `text`, so it's recorded as a BORROW extent instead
                // of being copied. Triggers are ASCII so the run never
                // splits a multibyte char, and stop delimiters start with
                // `*`/`_` (triggers), so a run never skips a pending
                // emphasis close.
                let start = i;
                // bytes[i] is non-trigger (this arm); find the next one.
                let run_end = match next_trigger(&bytes[i + 1..]) {
                    Some(off) => i + 1 + off,
                    None => bytes.len(),
                };
                // Hard line break: ≥2 spaces immediately before a `\n`
                // become a `<br />` (kramdown), consuming exactly two
                // spaces. Split the run at each such break; the `\n` itself
                // stays as joined text. (The backslash form `\<nl>` is a
                // trigger and is handled in the `\\` arm.)
                let mut seg = start;
                let mut p = start;
                while p < run_end {
                    if bytes[p] == b'\n' {
                        let mut sp = 0;
                        while p > start + sp && bytes[p - 1 - sp] == b' ' {
                            sp += 1;
                        }
                        if sp >= 2 {
                            acc.push_verbatim(seg, p - 2);
                            acc.flush(ast, &mut chain);
                            let br = ast.push_span(SpanKind::Raw(Cow::Borrowed("<br />")));
                            chain.link(br, |q, n| ast.spans[q as usize].next = Some(n));
                            seg = p; // drop the two spaces; keep the newline
                        }
                    }
                    p += 1;
                }
                acc.push_verbatim(seg, run_end);
                prev = text[seg..run_end].chars().next_back();
                i = run_end;
            }
        }
    }
    acc.flush(ast, &mut chain);
    Ok((chain.first(), None))
}

/// kramdown's intra-word underscore bail:
/// `pre_match =~ /[[:alpha:]]-?[[:alpha:]]*_*\z/`. `pre` is the local
/// slice before the delimiter; at a recursion boundary (`pre` empty)
/// the cross-span `prev` char approximates the lookback.
fn underscore_intraword(pre: &str, prev: Option<char>) -> bool {
    if pre.is_empty() {
        return prev.is_some_and(|c| c.is_alphabetic());
    }
    let s = pre.trim_end_matches('_');
    let s2 = s.trim_end_matches(|c: char| c.is_alphabetic());
    if s2.len() < s.len() {
        return true; // …alpha(_*)\z
    }
    if let Some(before_dash) = s2.strip_suffix('-') {
        // …alpha-\z (the optional hyphen with empty trailing alphas)
        return before_dash
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphabetic());
    }
    false
}

/// Bytes that begin a span-parser match arm (markup delimiters,
/// typography triggers, escapes). Everything else is ordinary text and
/// can be bulk-copied. The set MUST stay in sync with the `match c`
/// arms in `parse_spans_until` — e.g. `~`/`{` are here so a run never
/// swallows a `~~`/`{:` that should decline, and they're all ASCII so a
/// run never splits a multibyte char.
/// 256-entry membership table for the trigger bytes. One indexed load
/// per byte in the inline parser's hot "skip ordinary text" loop, vs a
/// chain of compares for 15 scattered values.
static TRIGGER: [bool; 256] = {
    let mut t = [false; 256];
    let mut i = 0;
    let set = b"\\`*_[!<>&~{'\"-.";
    while i < set.len() {
        t[set[i] as usize] = true;
        i += 1;
    }
    t
};

#[inline]
fn is_trigger(c: u8) -> bool {
    TRIGGER[c as usize]
}

/// Index of the first trigger byte in `hay`, or `None`. Scalar (the
/// `TRIGGER` table) by default; under `--features simd` on aarch64 a NEON
/// byteset scans 16 bytes per iteration. The two paths MUST agree — the
/// `next_trigger_matches_scalar` test pins it.
#[inline]
fn next_trigger(hay: &[u8]) -> Option<usize> {
    #[cfg(all(target_arch = "aarch64", feature = "simd"))]
    {
        // SAFETY: bounded 16-byte loads (guarded by `+ 16 <= len`); NEON
        // is baseline on aarch64.
        unsafe { next_trigger_neon(hay) }
    }
    #[cfg(not(all(target_arch = "aarch64", feature = "simd")))]
    {
        hay.iter().position(|&b| TRIGGER[b as usize])
    }
}

// NEON byteset (Langdale's nibble-lookup): a byte `b` is in the trigger
// set iff bit `b>>4` is set in LO_NIB[b & 0xF]. HI_NIB[h] = 1<<h selects
// that bit. High nibbles 8..15 (non-ASCII) map to 0 — never a trigger,
// so multibyte UTF-8 is skipped as ordinary run text.
#[cfg(all(target_arch = "aarch64", feature = "simd"))]
const LO_NIB: [u8; 16] = {
    let mut t = [0u8; 16];
    let set = b"\\`*_[!<>&~{'\"-.";
    let mut i = 0;
    while i < set.len() {
        let b = set[i];
        t[(b & 0x0F) as usize] |= 1u8 << (b >> 4);
        i += 1;
    }
    t
};
#[cfg(all(target_arch = "aarch64", feature = "simd"))]
const HI_NIB: [u8; 16] = {
    let mut t = [0u8; 16];
    let mut h = 0;
    while h < 8 {
        t[h] = 1u8 << h;
        h += 1;
    }
    t
};

#[cfg(all(target_arch = "aarch64", feature = "simd"))]
#[target_feature(enable = "neon")]
unsafe fn next_trigger_neon(hay: &[u8]) -> Option<usize> {
    use core::arch::aarch64::*;
    let lo_tbl = unsafe { vld1q_u8(LO_NIB.as_ptr()) };
    let hi_tbl = unsafe { vld1q_u8(HI_NIB.as_ptr()) };
    let mut i = 0;
    while i + 16 <= hay.len() {
        let v = unsafe { vld1q_u8(hay.as_ptr().add(i)) };
        let lo = vqtbl1q_u8(lo_tbl, vandq_u8(v, vdupq_n_u8(0x0F)));
        let hi = vqtbl1q_u8(hi_tbl, vshrq_n_u8(v, 4));
        // 0xFF in lanes where (lo & hi) != 0, i.e. byte is a trigger.
        let m = vtstq_u8(lo, hi);
        // NEON movemask: shift-narrow to 4 bits per lane → one nibble per
        // input byte in a u64; trailing_zeros/4 is the first match index.
        let narrowed = vshrn_n_u16(vreinterpretq_u16_u8(m), 4);
        let mask = vget_lane_u64(vreinterpret_u64_u8(narrowed), 0);
        if mask != 0 {
            return Some(i + (mask.trailing_zeros() as usize >> 2));
        }
        i += 16;
    }
    while i < hay.len() {
        if TRIGGER[hay[i] as usize] {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parsed-tree text the way kramdown-parser-gfm's `update_raw_text`
/// collects it: text and codespan values verbatim (typography already
/// applied in our Text spans), other elements contribute their
/// children's text (so link TEXT counts, the href doesn't). Walks the
/// flat arena from the chain head `spans`.
pub(crate) fn spans_raw_text(ast: &Ast<'_>, spans: Option<u32>, out: &mut String) {
    let mut cur = spans;
    while let Some(idx) = cur {
        let node = &ast.spans[idx as usize];
        match &node.kind {
            SpanKind::Text(t) | SpanKind::Code(t) => out.push_str(t),
            // Raw inline HTML contributes nothing to a heading's slug text.
            SpanKind::Raw(_) => {}
            // kramdown's GFM header-id slug ignores an image's alt text.
            SpanKind::Image { .. } => {}
            SpanKind::Em(inner)
            | SpanKind::Strong(inner)
            | SpanKind::Del(inner)
            | SpanKind::Link { spans: inner, .. } => {
                spans_raw_text(ast, *inner, out);
            }
        }
        cur = node.next;
    }
}

fn run_len(bytes: &[u8], i: usize, c: u8) -> usize {
    bytes[i..].iter().take_while(|b| **b == c).count()
}

/// Normalize a link-reference id the way kramdown does: lowercase, with
/// internal whitespace runs collapsed to one space and the ends trimmed.
fn normalize_ref_id(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = true; // also trims leading whitespace
    for c in s.chars() {
        if c.is_whitespace() {
            prev_ws = true;
        } else {
            if prev_ws && !out.is_empty() {
                out.push(' ');
            }
            out.extend(c.to_lowercase());
            prev_ws = false;
        }
    }
    out
}

/// Parse an HTML entity at the start of `s` (`s` begins with `&`), returning
/// its resolved code point and the byte length consumed (through the `;`).
/// `None` ⇒ not a recognized entity, so the caller leaves the `&` literal.
/// A well-formed numeric reference that overflows / is out of range still
/// returns `Some` with an invalid code point so the caller declines (rather
/// than mis-emitting a literal `&`, which kramdown would not).
pub(crate) fn parse_entity_at(s: &str) -> Option<(u32, usize)> {
    parse_entity(s)
}

fn parse_entity(s: &str) -> Option<(u32, usize)> {
    let b = s.as_bytes();
    debug_assert_eq!(b.first(), Some(&b'&'));
    if b.get(1) == Some(&b'#') {
        let (radix, start) = match b.get(2) {
            Some(&b'x') | Some(&b'X') => (16u32, 3),
            _ => (10u32, 2),
        };
        let mut j = start;
        let mut val: u64 = 0;
        while let Some(&d) = b.get(j) {
            if d == b';' {
                break;
            }
            let digit = (d as char).to_digit(radix)?; // non-digit ⇒ literal `&`
            val = val.saturating_mul(radix as u64).saturating_add(digit as u64);
            j += 1;
        }
        // Need at least one digit and a closing `;`.
        if j == start || b.get(j) != Some(&b';') {
            return None;
        }
        // Apply the C1 numeric remap; out-of-range stays out of range so the
        // caller's `char::from_u32` rejects it and the document declines.
        let cp = if val <= 0x10_FFFF {
            crate::entities::remap_numeric(val as u32)
        } else {
            0x11_0000
        };
        Some((cp, j + 1))
    } else {
        let mut j = 1;
        while let Some(&d) = b.get(j) {
            if d == b';' {
                break;
            }
            if !d.is_ascii_alphanumeric() {
                return None; // names are alphanumeric in kramdown's table
            }
            j += 1;
        }
        if j == 1 || b.get(j) != Some(&b';') {
            return None;
        }
        let cp = crate::entities::named(&s[1..j])?;
        Some((cp, j + 1))
    }
}

/// A trailing `"…"` / `'…'` title in a link definition: `s` begins with the
/// quote, and the matching quote must be followed only by whitespace (the
/// `(?:…(["'])(.+?)\4)?[ \t]*?\n` tail of kramdown's regex). `None` if `s`
/// isn't a well-formed quoted-to-end title. (`(…)` is NOT a title in a
/// definition — only inline links allow that — so the bare-dest caller keeps
/// parens in the destination.)
fn parse_def_title(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let q = *b.first()?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    // Non-greedy `.+?`: the first closing quote (content ≥ 1 char) after
    // which only whitespace remains.
    let mut j = 2; // content must be at least one byte
    while j < b.len() {
        if b[j] == q && s[j + 1..].bytes().all(|c| c == b' ' || c == b'\t') {
            return Some(&s[1..j]);
        }
        j += 1;
    }
    None
}

/// Parse a link reference definition: `[id]: dest`, the destination bare or
/// in `<…>`, with an optional trailing `"title"` / `'title'`, behind ≤3
/// spaces. Mirrors kramdown's `LINK_DEFINITION_START` + its post-check:
/// the bare destination may contain spaces (so an unexpanded Liquid
/// `{{ … }}` URL parses) but NOT whitespace-then-quote — kramdown rejects
/// such a line, so we do too. `None` unless the whole line is a clean
/// single-line definition.
fn parse_link_def(line: &str) -> Option<(String, &str, Option<&str>)> {
    let lead = line.len() - line.trim_start_matches(' ').len();
    if lead > 3 {
        return None; // ≥4 spaces is indented code, not a definition
    }
    let t = line[lead..].strip_prefix('[')?;
    // `[^id]:` is a FOOTNOTE definition, not a link definition — leave it
    // for the footnote path (which declines), don't swallow it here.
    if t.starts_with('^') {
        return None;
    }
    let close = t.find(']')?;
    let id = normalize_ref_id(&t[..close]);
    if id.is_empty() {
        return None;
    }
    // After `]:`, kramdown's `[ \t]*` eats leading and its `[ \t]*?\n` eats
    // trailing whitespace, so trim both ends of the run we examine.
    let rest = t[close + 1..].strip_prefix(':')?.trim_matches([' ', '\t']);
    if rest.is_empty() {
        return None;
    }

    if let Some(r) = rest.strip_prefix('<') {
        // Angle destination `<url>`, optional trailing title.
        let end = r.find('>')?;
        let after = r[end + 1..].trim_start_matches([' ', '\t']);
        let title = if after.is_empty() {
            None
        } else {
            Some(parse_def_title(after)?)
        };
        return Some((id, &r[..end], title));
    }

    // Bare destination: spaces allowed. The first whitespace-then-quote
    // either STARTS a valid trailing title (everything before it is the
    // destination) or makes the line invalid — kramdown's post-check
    // `return false if dest =~ /[ \t]+["']/` rejects a destination that
    // itself contains whitespace-then-quote.
    let mut q = 1;
    let bytes = rest.as_bytes();
    while q < bytes.len() {
        if (bytes[q] == b'"' || bytes[q] == b'\'') && matches!(bytes[q - 1], b' ' | b'\t') {
            // Destination is everything before this whitespace run.
            let dest_end = rest[..q].trim_end_matches([' ', '\t']).len();
            if dest_end == 0 {
                return None; // destination needs a non-space char
            }
            let title = parse_def_title(&rest[q..])?; // else: not a definition
            return Some((id, &rest[..dest_end], Some(title)));
        }
        q += 1;
    }
    Some((id, rest, None)) // whole line is the destination, no title
}

/// Whether `line` has the block link-definition SHAPE `[id]:` (≤3-space
/// indent, non-empty bracket id, then a colon) — regardless of whether the
/// destination/title parse cleanly. Used to decline a definition kramdown
/// would accept but [`parse_link_def`] can't reproduce (e.g. a destination
/// with embedded spaces from an unexpanded Liquid `{{ … }}`).
fn looks_like_link_def(line: &str) -> bool {
    let lead = line.len() - line.trim_start_matches(' ').len();
    if lead > 3 {
        return false;
    }
    let Some(t) = line[lead..].strip_prefix('[') else {
        return false;
    };
    // `[^id]:` is a footnote definition, handled (declined) elsewhere.
    if t.starts_with('^') {
        return false;
    }
    let Some(close) = t.find(']') else {
        return false;
    };
    !t[..close].trim_matches([' ', '\t']).is_empty() && t[close + 1..].starts_with(':')
}

/// Pre-pass: collect block-level link reference definitions and mark each
/// definition's line for removal from the block stream. A definition is
/// recognized only at a block boundary (document start, after a blank, or
/// after another definition); a `[id]: url`-looking line in the middle of
/// a paragraph stays literal text.
///
/// A boundary line that has the definition SHAPE but doesn't parse declines
/// the document: kramdown would still lift it as a definition (so the
/// surrounding blanks collapse and any `[text][id]` resolves), and emitting
/// it as a literal paragraph instead would be byte-wrong.
fn collect_link_defs<'a>(lines: &[&'a str]) -> Result<(LinkDefs<'a>, Vec<bool>), Error> {
    let mut map: LinkDefs<'a> = HashMap::new();
    let mut mask = vec![false; lines.len()];
    let mut at_boundary = true;
    for (idx, &line) in lines.iter().enumerate() {
        if is_blank(line) {
            at_boundary = true;
            continue;
        }
        if at_boundary {
            if let Some((id, url, title)) = parse_link_def(line) {
                map.entry(id).or_insert((url, title)); // first definition wins
                mask[idx] = true;
                continue; // stay at a boundary: consecutive defs all count
            }
            if looks_like_link_def(line) {
                return Err(declined("link-definition"));
            }
        }
        at_boundary = false;
    }
    Ok((map, mask))
}

/// Parse `![alt](src "title")` at the start of `rest` (`rest` begins with
/// `![`). `alt` is the RAW bracket text — kramdown keeps markup literal in
/// the alt attribute. Anything that isn't a clean inline image yields
/// `None` so the caller renders `!` literally (reference-style images need
/// a definition, and a doc that has one already declines).
#[allow(clippy::type_complexity)]
fn parse_image<'a>(
    ast: &Ast<'a>,
    rest: &'a str,
) -> Result<Option<(Cow<'a, str>, Cow<'a, str>, Option<Cow<'a, str>>, usize)>, Error> {
    let bytes = rest.as_bytes();
    debug_assert!(bytes.starts_with(b"!["));
    // Closing `]` of the alt. Escaped / nested brackets are out of subset.
    let mut close = None;
    let mut k = 2;
    while k < bytes.len() {
        match bytes[k] {
            b'\\' => return Err(declined("image-alt-escape")),
            b'[' => return Err(declined("image-alt-nested")),
            b']' => {
                close = Some(k);
                break;
            }
            _ => {}
        }
        k += 1;
    }
    let Some(close) = close else {
        return Ok(None);
    };
    let alt = &rest[2..close];
    // `]` not followed by `(`: reference image (full / collapsed /
    // shortcut) resolved against the collected definitions, else literal.
    if bytes.get(close + 1) != Some(&b'(') {
        let (id, consumed) = match bytes.get(close + 1) {
            Some(b'[') => {
                let inner = &rest[close + 2..];
                let Some(idc) = inner.find(']') else {
                    return Ok(None);
                };
                let explicit = &inner[..idc];
                let id = if explicit.trim_matches([' ', '\t']).is_empty() {
                    normalize_ref_id(alt)
                } else {
                    normalize_ref_id(explicit)
                };
                (id, close + 2 + idc + 1)
            }
            _ => {
                let tail = &rest[close + 1..];
                if tail.starts_with([' ', '\t']) && tail.trim_start().starts_with('[') {
                    return Err(declined("ref-space-separated"));
                }
                (normalize_ref_id(alt), close + 1)
            }
        };
        let Some(&(src, title)) = ast.link_defs.get(&id) else {
            return Ok(None);
        };
        return Ok(Some((
            Cow::Borrowed(src),
            Cow::Borrowed(alt),
            title.map(Cow::Borrowed),
            consumed,
        )));
    }
    let after = &rest[close + 2..];
    let Some(paren_rel) = after.find(')') else {
        return Ok(None);
    };
    let Some((src, title)) = split_href_title(&after[..paren_rel]) else {
        return Ok(None);
    };
    if src.contains('(') || src.starts_with('<') {
        return Err(declined("image-dest"));
    }
    Ok(Some((
        Cow::Borrowed(src),
        Cow::Borrowed(&rest[2..close]),
        title.map(Cow::Borrowed),
        close + 2 + paren_rel + 1,
    )))
}

/// Parse `[text](href)` at the start of `rest`. Titles, references and
/// nested brackets decline. The enclosing-emphasis flags thread through
/// so same-type nesting stays blocked inside link text (kramdown's
/// `@stack` check spans the link boundary).
#[allow(clippy::type_complexity)]
fn parse_link<'a>(
    ast: &mut Ast<'a>,
    rest: &'a str,
    in_em: bool,
    in_strong: bool,
) -> Result<Option<(Option<u32>, Cow<'a, str>, Option<Cow<'a, str>>, usize)>, Error> {
    let bytes = rest.as_bytes();
    debug_assert_eq!(bytes[0], b'[');
    // Find the closing `]`. Escaped brackets (`\]`/`\[`) and nested
    // brackets in the link text are special in kramdown (escapes, nested
    // links rendered literally); rather than risk wrong output we decline
    // so the document falls back — the same right-or-declined result these
    // produced before inline-link text was widened.
    // Find the `]` that closes the link text, tracking bracket depth so
    // balanced nested brackets (`[text [x] y]`, a linked image `[![…](…)]`)
    // stay inside the text, skipping code spans (a `[` in `` `…` `` is
    // literal) and escaped chars (`\]`). `parse_spans_until` then renders the
    // text — including any nested image, code span, or literal brackets.
    let mut close = None;
    let mut depth = 0u32;
    let mut k = 1;
    while k < bytes.len() {
        match bytes[k] {
            b'\\' => {
                k += 2; // skip the escaped char
                continue;
            }
            b'`' => {
                // Skip a balanced code span (matching backtick-run lengths).
                let run = run_len(bytes, k, b'`');
                let mut j = k + run;
                let mut closed = false;
                while j < bytes.len() {
                    if bytes[j] == b'`' {
                        let r2 = run_len(bytes, j, b'`');
                        if r2 == run {
                            j += run;
                            closed = true;
                            break;
                        }
                        j += r2;
                    } else {
                        j += 1;
                    }
                }
                k = if closed { j } else { k + run };
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                if depth == 0 {
                    close = Some(k);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
        k += 1;
    }
    // Unclosed bracket: not a link — the caller emits `[` literally.
    let Some(close) = close else {
        return Ok(None);
    };
    // `]` not followed by `(`: try a reference link (full / collapsed /
    // shortcut) against the collected definitions, else literal `[`.
    if bytes.get(close + 1) != Some(&b'(') {
        return resolve_ref_link(ast, rest, close, in_em, in_strong);
    }
    let after = &rest[close + 2..];
    let Some(paren_rel) = after.find(')') else {
        return Ok(None);
    };
    let Some((href, title)) = split_href_title(&after[..paren_rel]) else {
        // Quote present but the title is malformed (e.g. `url "t" extra`):
        // kramdown treats the whole `[…](…)` as literal text.
        return Ok(None);
    };
    // Balanced parens in a URL (`…/Foo_(bar)`) and angle-bracket
    // destinations (`<with spaces>`) need kramdown's quirkier dest scan
    // (it even keeps an unbalanced `(` with a following title) — the cheap
    // first-`)` scan can't reproduce them, so decline rather than truncate.
    if href.contains('(') || href.starts_with('<') {
        return Err(declined("link-dest"));
    }
    let (spans, _) =
        parse_spans_until(ast, &rest[1..close], None, in_em, in_strong, Some(Elem::Link))?;
    // `href`/`title` are sub-slices of `rest` (the link source) — borrow.
    Ok(Some((
        spans,
        Cow::Borrowed(href),
        title.map(Cow::Borrowed),
        close + 2 + paren_rel + 1,
    )))
}

/// Resolve a reference link at `rest` whose text closes at `close` (the
/// `]`), which is NOT followed by `(`: full `[text][id]`, collapsed
/// `[text][]`, or shortcut `[text]`. Looks the normalized id up in the
/// collected definitions; an undefined reference yields `None` (literal
/// `[`). The `] [id]` space-separated full form is rare — decline it.
#[allow(clippy::type_complexity)]
fn resolve_ref_link<'a>(
    ast: &mut Ast<'a>,
    rest: &'a str,
    close: usize,
    in_em: bool,
    in_strong: bool,
) -> Result<Option<(Option<u32>, Cow<'a, str>, Option<Cow<'a, str>>, usize)>, Error> {
    let bytes = rest.as_bytes();
    let text = &rest[1..close];
    let (id, consumed) = match bytes.get(close + 1) {
        Some(b'[') => {
            // full `[text][id]` / collapsed `[text][]`
            let inner = &rest[close + 2..];
            let Some(idc) = inner.find(']') else {
                return Ok(None);
            };
            let explicit = &inner[..idc];
            let id = if explicit.trim_matches([' ', '\t']).is_empty() {
                normalize_ref_id(text)
            } else {
                normalize_ref_id(explicit)
            };
            (id, close + 2 + idc + 1)
        }
        _ => {
            // shortcut `[text]`; `] <ws> [` is the space-separated full
            // form (out of subset) — decline rather than mis-resolve.
            let tail = &rest[close + 1..];
            if tail.starts_with([' ', '\t']) && tail.trim_start().starts_with('[') {
                return Err(declined("ref-space-separated"));
            }
            (normalize_ref_id(text), close + 1)
        }
    };
    let Some(&(href, title)) = ast.link_defs.get(&id) else {
        return Ok(None);
    };
    let (spans, _) =
        parse_spans_until(ast, text, None, in_em, in_strong, Some(Elem::Link))?;
    Ok(Some((
        spans,
        Cow::Borrowed(href),
        title.map(Cow::Borrowed),
        consumed,
    )))
}

/// Split a link destination `(…)` body into `(href, optional title)`,
/// matching kramdown: `url`, `url "title"`, `url 'title'`. A bare space
/// with no quote stays in the href (`url with space` ⇒ that whole href).
/// Returns `None` when a quote is present but the title is malformed —
/// kramdown then declines the link and the `[` is rendered literally.
fn split_href_title(inner: &str) -> Option<(&str, Option<&str>)> {
    let bytes = inner.as_bytes();
    let Some(&last) = bytes.last() else {
        return Some((inner, None)); // empty `()`
    };
    if last != b'"' && last != b'\'' {
        // No trailing quote: a stray quote elsewhere ⇒ malformed title.
        if inner.contains('"') || inner.contains('\'') {
            return None;
        }
        // kramdown trims surrounding whitespace from the destination.
        return Some((inner.trim_matches([' ', '\t']), None));
    }
    // Closing quote is the last byte; the opening quote is the leftmost
    // matching quote preceded by ASCII whitespace (the `url "title"` form).
    let q = last;
    let mut open = None;
    for j in 1..inner.len().saturating_sub(1) {
        if bytes[j] == q && bytes[j - 1].is_ascii_whitespace() {
            open = Some(j);
            break;
        }
    }
    let open = open?;
    let href = inner[..open].trim_matches([' ', '\t']);
    let title = &inner[open + 1..inner.len() - 1];
    // No nested same-quote in the title, and the href must be quote-free.
    if title.as_bytes().contains(&q) || href.contains('"') || href.contains('\'') {
        return None;
    }
    Some((href, Some(title)))
}

#[cfg(test)]
mod byte_opt_tests {
    //! Unit coverage for the byte-scan rewrites (perf work). The golden
    //! corpus gates byte-identity at the document level; these pin the
    //! individual functions on edge cases the corpus may not exercise —
    //! especially the byte-vs-char hazards (escaped/After-multibyte `|`).
    use super::*;

    fn reason(line: &str) -> Option<&'static str> {
        match decline_block_scan(line) {
            Ok(()) => None,
            Err(Error::Declined(r)) => Some(r),
        }
    }

    #[test]
    fn table_pipe_escape_and_multibyte() {
        // The table-trigger pipe scan (now consulted by the block loop and
        // list-item parser, not `decline_block_scan`).
        assert!(!has_table_pipe("plain prose, nothing special"));
        assert!(has_table_pipe("a | b")); // unescaped pipe
        assert!(!has_table_pipe(r"a \| b")); // escaped pipe is NOT a table
        assert!(has_table_pipe(r"a \\| b")); // \\ then | → unescaped
        // byte scan must stay correct around multibyte chars:
        assert!(has_table_pipe("café | x"));
        assert!(!has_table_pipe(r"café \| x"));
        assert!(!has_table_pipe("naïve prose")); // multibyte, no pipe
        // A `|` inside a balanced code span is literal, not a table.
        assert!(!has_table_pipe("prose `arr.each { |x| x }` here"));
        assert!(!has_table_pipe("`|`")); // a lone piped code span
        assert!(has_table_pipe("`code` | real")); // top-level pipe
        // `decline_block_scan` no longer declines a lone pipe line.
        assert_eq!(reason("a | b"), None);
    }

    #[test]
    fn decline_indented_code_and_prefixes() {
        assert_eq!(reason("    four spaces"), Some("indented-code"));
        assert_eq!(reason("\ttab"), Some("indented-code"));
        assert_eq!(reason("   three spaces ok"), None);
        assert_eq!(reason("{:.css}"), Some("ald-ial-extension"));
        assert_eq!(reason("[^1]: footnote"), Some("footnote"));
        assert_eq!(reason("$$ math $$"), Some("math"));
        assert_eq!(reason("<div>"), Some("html-block"));
        // Link reference definitions are lifted out in a pre-pass, not
        // declined here; a def-shaped line that reaches the block scan is
        // mid-paragraph and stays literal text.
        assert_eq!(reason("[id]: http://x"), None);
    }

    #[test]
    fn link_def_shape_detection() {
        // Parseable definitions and clean shapes.
        assert!(looks_like_link_def("[id]: http://x"));
        assert!(looks_like_link_def("   [id]: http://x")); // ≤3-space indent
        // The real-world blocker: an unexpanded Liquid `{{ … }}` puts a
        // space in the destination, so `parse_link_def` fails — but the
        // SHAPE matches, so the document declines instead of emitting the
        // line as a literal paragraph.
        assert!(looks_like_link_def("[1349]: {{ site.repository }}/issues/1349"));
        // An unexpanded Liquid `{{ … }}` URL has spaces but no whitespace+
        // quote, so it parses (kramdown's bare destination allows spaces).
        let (id, url, title) =
            parse_link_def("[1349]: {{ site.repository }}/issues/1349").unwrap();
        assert_eq!((id.as_str(), url, title), ("1349", "{{ site.repository }}/issues/1349", None));
        // A trailing quoted title splits off the destination.
        assert_eq!(parse_link_def("[a]: dest \"t\""), Some(("a".into(), "dest", Some("t"))));
        // `(…)` is NOT a title in a definition — it stays in the destination.
        assert_eq!(parse_link_def("[a]: dest (x)"), Some(("a".into(), "dest (x)", None)));
        // Whitespace-then-quote inside the would-be destination ⇒ not a def.
        assert_eq!(parse_link_def("[a]: a b \"t\" extra"), None);
        // Footnote definitions are not link definitions.
        assert!(!looks_like_link_def("[^first]: note"));
        assert_eq!(parse_link_def("[^first]: note"), None);
        // Not definitions: inline link, mid-line, empty id, deep indent.
        assert!(!looks_like_link_def("[text](url)"));
        assert!(!looks_like_link_def("see [id]: later"));
        assert!(!looks_like_link_def("[]: http://x"));
        assert!(!looks_like_link_def("    [id]: http://x")); // 4 spaces = code
    }

    #[test]
    fn is_hr_true_cases() {
        for s in ["---", "***", "___", "----", "- - -", "*  *  *", "  ---  ", "-\t-\t-"] {
            assert!(is_hr(s), "{s:?} should be HR");
        }
    }

    #[test]
    fn is_hr_false_cases() {
        for s in ["--", "**", "hello", "-*-", "- - x", "", "- -", "-x-", "a---", "---x"] {
            assert!(!is_hr(s), "{s:?} should NOT be HR");
        }
    }

    #[test]
    fn split_lines_matches_std_split() {
        for s in [
            "", "a", "a\n", "a\nb", "a\nb\n", "\n", "\n\n", "a\n\nb", "café\nx\n",
            "trailing\nnewline\n", "no newline at all",
        ] {
            let std: Vec<&str> = s.split('\n').collect();
            assert_eq!(split_lines(s), std, "split mismatch for {s:?}");
        }
    }

    #[test]
    fn next_trigger_matches_scalar() {
        // Every value 0..=255 (incl. non-ASCII, which must NOT match) at
        // every length — exercises the NEON path under `--features simd`
        // against the scalar `is_trigger` oracle.
        let bytes: Vec<u8> = (0u8..=255).cycle().take(400).collect();
        for len in 0..bytes.len() {
            let hay = &bytes[..len];
            let oracle = hay.iter().position(|&b| is_trigger(b));
            assert_eq!(next_trigger(hay), oracle, "len={len}");
        }
        for pos in 0..40usize {
            let mut h = vec![b'x'; 40];
            h[pos] = b'*';
            assert_eq!(next_trigger(&h), Some(pos), "pos={pos}");
        }
    }

    #[test]
    fn split_href_title_cases() {
        // (verified byte-identical against kramdown 2.5.2)
        assert_eq!(split_href_title("url"), Some(("url", None)));
        assert_eq!(
            split_href_title(r#"http://x.com "title""#),
            Some(("http://x.com", Some("title")))
        );
        assert_eq!(
            split_href_title("http://x.com 'sgl'"),
            Some(("http://x.com", Some("sgl")))
        );
        // bare space with no quote stays in the href
        assert_eq!(
            split_href_title("url with space"),
            Some(("url with space", None))
        );
        // empty href + title
        assert_eq!(split_href_title(r#" "t""#), Some(("", Some("t"))));
        // kramdown trims surrounding whitespace from the destination
        assert_eq!(split_href_title("  spaces.html  "), Some(("spaces.html", None)));
        // malformed: quote present, trailing junk after the close quote
        assert_eq!(split_href_title(r#"url "t" extra"#), None);
    }

    #[test]
    fn parse_image_cases() {
        // (verified byte-identical against kramdown 2.5.2)
        let ast = Ast::new(); // empty defs ⇒ a reference image is undefined
        let (src, alt, title, _) = parse_image(&ast, "![alt](/i.png)").unwrap().unwrap();
        assert_eq!((&*src, &*alt, title.as_deref()), ("/i.png", "alt", None));

        let (src, alt, title, _) = parse_image(&ast, r#"![a](/i.png "t")"#).unwrap().unwrap();
        assert_eq!((&*src, &*alt, title.as_deref()), ("/i.png", "a", Some("t")));

        let (src, alt, _, _) = parse_image(&ast, "![](u)").unwrap().unwrap();
        assert_eq!((&*src, &*alt), ("u", ""));

        // raw alt keeps markup literal (kramdown does not parse it)
        let (_, alt, _, _) = parse_image(&ast, "![a *b* c](u)").unwrap().unwrap();
        assert_eq!(&*alt, "a *b* c");

        // reference-style image with no matching definition ⇒ not inline
        assert!(parse_image(&ast, "![a][r]").unwrap().is_none());
    }

    #[test]
    fn parse_ial_cases() {
        // (verified byte-identical against kramdown 2.5.2)
        let pairs = |s: &str| {
            parse_ial(s).map(|v| {
                v.into_iter()
                    .map(|(k, val)| (k.into_owned(), val))
                    .collect::<Vec<_>>()
            })
        };
        let p = |kv: &[(&str, &str)]| {
            Some(
                kv.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(pairs(".note"), p(&[("class", "note")]));
        assert_eq!(pairs(".a.b.c"), p(&[("class", "a b c")])); // dot-concat
        assert_eq!(pairs(".a .b #i"), p(&[("class", "a b"), ("id", "i")]));
        // insertion order is preserved (id before class here)
        assert_eq!(pairs("#i .c"), p(&[("id", "i"), ("class", "c")]));
        assert_eq!(
            pairs(r#"title="x" .c #i"#),
            p(&[("title", "x"), ("class", "c"), ("id", "i")])
        );
        assert_eq!(pairs(r#"rel="noopener""#), p(&[("rel", "noopener")]));
        assert_eq!(pairs("toc"), None); // bare name (ALD ref) ⇒ decline
        assert_eq!(pairs(""), None);
    }

    #[test]
    fn link_def_and_ref_id() {
        // (verified byte-identical against kramdown 2.5.2)
        assert_eq!(normalize_ref_id("  A  B  "), "a b");
        assert_eq!(parse_link_def("[id]: /url"), Some(("id".into(), "/url", None)));
        assert_eq!(
            parse_link_def(r#"[A B]: /u "t""#),
            Some(("a b".into(), "/u", Some("t")))
        );
        assert_eq!(
            parse_link_def("[a]: <http://x.com>"),
            Some(("a".into(), "http://x.com", None))
        );
        assert_eq!(parse_link_def("  [a]: /u"), Some(("a".into(), "/u", None))); // ≤3 indent
        assert_eq!(parse_link_def("    [a]: /u"), None); // 4 spaces ⇒ code
        assert_eq!(parse_link_def("not a def"), None);
    }

    #[test]
    fn table_cell_split_cases() {
        // (verified byte-identical against kramdown 2.5.2)
        assert_eq!(split_table_cells("a | b"), Some(vec!["a", "b"]));
        assert_eq!(split_table_cells("| a | b |"), Some(vec!["a", "b"]));
        assert_eq!(split_table_cells("| a |  | c |"), Some(vec!["a", "", "c"]));
        // escaped pipe is not a boundary (the cell keeps `\|`)
        assert_eq!(split_table_cells(r"x | y \| z | w"), Some(vec!["x", r"y \| z", "w"]));
        // a pipe inside a balanced code span is literal, not a boundary
        assert_eq!(split_table_cells("`a|b` | c"), Some(vec!["`a|b`", "c"]));
        // no top-level pipe ⇒ not a table row
        assert_eq!(split_table_cells("just prose"), None);
        // unbalanced code span ⇒ out of subset
        assert_eq!(split_table_cells("`a | b"), None);
    }

    #[test]
    fn table_sep_line_cases() {
        assert!(is_table_sep_line("---|---"));
        assert!(is_table_sep_line("|:--|--:|"));
        assert!(is_table_sep_line(" :-: | -: "));
        assert!(is_table_sep_line("|-----"));
        assert!(!is_table_sep_line("a | b")); // a data row, not a separator
        assert!(!is_table_sep_line("::|::")); // no dash
        assert!(!is_table_sep_line("")); // empty
    }
}
