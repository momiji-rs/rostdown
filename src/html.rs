//! HTML conversion, byte-faithful to kramdown's HTML converter for the
//! supported subset: one `"\n"` per blank-run between blocks, child
//! blocks indented by 2, `auto_ids` slugs with kramdown's exact rules,
//! `<pre><code class="language-…">` for plain code blocks and the
//! `<div class="language-… highlighter-…">` wrapper for highlighted ones.
//!
//! Perf: the converter writes everything through `push_str`/`push` into
//! one pre-sized output buffer — no `format!` temporaries, no per-block
//! `" ".repeat()` pad strings, and `escape_*` copies non-special runs in
//! bulk. Output stays byte-identical (the golden corpus is the gate).

use std::borrow::Cow;
use std::collections::HashMap;

use crate::parse::{Align, Ast, BlockKind, SpanKind, TocEntry};
use crate::{CodeHighlighter, Options};

/// Run a highlighter callback with the bump arena paused (only when the
/// `arena` feature is active). A custom highlighter may stash data that
/// must outlive this render — e.g. a recording highlighter that captures
/// `(lang, code)` for a second pass (the shape the kramdown-rostdown gem
/// uses). Pausing routes the callback's allocations to the system
/// allocator so they survive `to_html`'s end-of-scope arena reset instead
/// of dangling. Zero-cost no-op without the feature.
#[cfg(feature = "arena")]
#[inline]
fn with_hl_paused<R>(f: impl FnOnce() -> R) -> R {
    let saved = crate::arena::pause();
    let r = f();
    crate::arena::resume(saved);
    r
}
#[cfg(not(feature = "arena"))]
#[inline]
fn with_hl_paused<R>(f: impl FnOnce() -> R) -> R {
    f()
}

pub(crate) fn convert(
    ast: &Ast<'_>,
    root: Option<u32>,
    opts: &Options,
    hl: &mut dyn CodeHighlighter,
    src_len: usize,
) -> String {
    // HTML for the supported subset runs ~1.2–1.5× the source; pre-size
    // to skip the geometric regrowth on a fresh String.
    let mut out = String::with_capacity(src_len + src_len / 2 + 64);
    let mut used_ids: HashMap<String, u32> = HashMap::new();
    convert_blocks(&mut out, ast, root, 0, opts, hl, &mut used_ids);
    out
}

/// Push `n` spaces without allocating (replaces `" ".repeat(n)`).
fn push_pad(out: &mut String, n: usize) {
    const SP: &str = "                                "; // 32 spaces
    let mut left = n;
    while left >= SP.len() {
        out.push_str(SP);
        left -= SP.len();
    }
    out.push_str(&SP[..left]);
}

/// `<h1>`..`<h6>`: heading levels are 1..=6, so the level digit is a
/// single ASCII char — cheaper than formatting an integer.
fn push_level_digit(out: &mut String, level: u8) {
    out.push((b'0' + level) as char);
}

fn convert_blocks(
    out: &mut String,
    ast: &Ast<'_>,
    head: Option<u32>,
    indent: usize,
    opts: &Options,
    hl: &mut dyn CodeHighlighter,
    used_ids: &mut HashMap<String, u32>,
) {
    let mut cur = head;
    while let Some(idx) = cur {
        let block = &ast.blocks[idx as usize];
        cur = block.next;
        match &block.kind {
            // kramdown emits a bare "\n" per blank-run, no indent.
            BlockKind::Blank => out.push('\n'),
            BlockKind::Heading {
                level,
                raw,
                span_text,
                spans,
            } => {
                push_pad(out, indent);
                out.push_str("<h");
                push_level_digit(out, *level);
                emit_attrs(out, &block.ial);
                // auto_ids supplies an id only when an IAL didn't set one.
                let ial_has_id = block.ial.iter().any(|(k, _)| k.as_ref() == "id");
                if opts.auto_ids && !ial_has_id {
                    // GFM sets ids at parse time with its own slug
                    // algorithm; core uses the converter's. Parse
                    // validated gfm_slug, so the fallback is inert.
                    let base = if opts.gfm {
                        gfm_slug(span_text).unwrap_or_else(|| basic_generate_id(raw))
                    } else {
                        basic_generate_id(raw)
                    };
                    let id = dedup_id(base, used_ids);
                    out.push_str(" id=\"");
                    out.push_str(&id);
                    out.push('"');
                }
                out.push('>');
                convert_spans(out, ast, *spans, hl.codespan_class());
                out.push_str("</h");
                push_level_digit(out, *level);
                out.push_str(">\n");
            }
            BlockKind::Para(spans) => {
                push_pad(out, indent);
                out.push_str("<p");
                emit_attrs(out, &block.ial);
                out.push('>');
                convert_spans(out, ast, *spans, hl.codespan_class());
                out.push_str("</p>\n");
            }
            BlockKind::List {
                ordered,
                loose,
                items,
                toc,
            } => {
                if *toc {
                    emit_toc(out, ast, &block.ial, *ordered, indent, hl);
                } else {
                    emit_list(
                        out, ast, *items, *ordered, *loose, &block.ial, indent, opts, hl, used_ids,
                    );
                }
            }
            BlockKind::Quote(inner) => {
                push_pad(out, indent);
                out.push_str("<blockquote");
                emit_attrs(out, &block.ial);
                out.push_str(">\n");
                convert_blocks(out, ast, *inner, indent + 2, opts, hl, used_ids);
                push_pad(out, indent);
                out.push_str("</blockquote>\n");
            }
            BlockKind::Code { lang, text } => {
                // kramdown resolves a missing fence language to
                // syntax_highlighter_opts[:default_lang]; the wrapper
                // class uses the same resolved value. Resolve to an owned
                // string so the `hl` borrow is released before the
                // `&mut hl.highlight(...)` call below (the fence lang —
                // the common case — borrows from `src` via the Cow).
                let effective: Option<std::borrow::Cow<'_, str>> = match lang {
                    Some(l) => Some(std::borrow::Cow::Borrowed(l.as_ref())),
                    None => hl.default_lang().map(|d| std::borrow::Cow::Owned(d.to_string())),
                };
                if let Some(hl_lang) = effective.as_deref()
                    && let Some(hl_html) = with_hl_paused(|| hl.highlight(hl_lang, text))
                {
                    // kramdown convert_codeblock with a syntax highlighter.
                    push_pad(out, indent);
                    out.push_str("<div class=\"language-");
                    out.push_str(hl_lang);
                    out.push_str(" highlighter-");
                    out.push_str(hl.name());
                    out.push_str("\">");
                    out.push_str(&hl_html);
                    push_pad(out, indent);
                    out.push_str("</div>\n");
                } else {
                    push_pad(out, indent);
                    // A leading block IAL (`{:.x}` directly above the fence)
                    // lands on `<pre>`; the language class stays on `<code>`.
                    out.push_str("<pre");
                    emit_attrs(out, &block.ial);
                    out.push_str("><code");
                    if let Some(lang) = lang {
                        out.push_str(" class=\"language-");
                        out.push_str(lang);
                        out.push('"');
                    }
                    out.push('>');
                    let body_start = out.len();
                    escape_text(out, text);
                    // kramdown: chomp, then exactly one trailing newline.
                    if !out[body_start..].ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("</code></pre>\n");
                }
            }
            BlockKind::Hr => {
                push_pad(out, indent);
                out.push_str("<hr />\n");
            }
            BlockKind::RawHtml { html, md_spans } => {
                // Already serialized to match kramdown's HTML converter
                // (parsed at column 0, top level). Each `\0` sentinel marks a
                // `markdown="1"` element's content — splice the rendered
                // markdown spans in their place.
                if md_spans.is_empty() {
                    out.push_str(html);
                } else {
                    let mut spans = md_spans.iter();
                    let mut first = true;
                    for segment in html.split('\0') {
                        if !first
                            && let Some(&head) = spans.next()
                        {
                            convert_spans(out, ast, head, hl.codespan_class());
                        }
                        out.push_str(segment);
                        first = false;
                    }
                }
                out.push('\n');
            }
            BlockKind::Table {
                aligns,
                header,
                body,
            } => emit_table(out, ast, aligns, header, body, &block.ial, indent, hl),
        }
    }
}

/// kramdown's `<table>` emission at a given base indent: `<thead>` (only when
/// a header row exists), `<tbody>`, and the rows two levels deeper. Shared by
/// the top-level table block and a pipe-table living inside a `<li>`. A
/// leading block IAL (`{: .note}` directly above the table) lands on `<table>`.
#[allow(clippy::too_many_arguments)]
fn emit_table(
    out: &mut String,
    ast: &Ast<'_>,
    aligns: &[Align],
    header: &Option<Vec<Option<u32>>>,
    body: &[Vec<Option<u32>>],
    ial: &[(Cow<'_, str>, String)],
    indent: usize,
    hl: &mut dyn CodeHighlighter,
) {
    push_pad(out, indent);
    out.push_str("<table");
    emit_attrs(out, ial);
    out.push_str(">\n");
    if let Some(header) = header {
        push_pad(out, indent + 2);
        out.push_str("<thead>\n");
        emit_table_row(out, ast, header, aligns, true, indent + 4, hl);
        push_pad(out, indent + 2);
        out.push_str("</thead>\n");
    }
    push_pad(out, indent + 2);
    out.push_str("<tbody>\n");
    for row in body {
        emit_table_row(out, ast, row, aligns, false, indent + 4, hl);
    }
    push_pad(out, indent + 2);
    out.push_str("</tbody>\n");
    push_pad(out, indent);
    out.push_str("</table>\n");
}

/// Emit a span element's IAL attributes (`[t](u){:.c}`), if any, from the
/// side table — used inside the open tag, after the element's own attrs.
#[inline]
fn emit_span_ial(out: &mut String, ast: &Ast<'_>, idx: u32) {
    // Most documents carry no span IALs at all; skip the per-span hash
    // lookup (called for every em/strong/link) when the side table is empty.
    if ast.span_ials.is_empty() {
        return;
    }
    if let Some(attrs) = ast.span_ials.get(&idx) {
        emit_attrs(out, attrs);
    }
}

/// Emit an IAL's attributes into an open tag: ` key="value"` per pair in
/// insertion order, the value HTML-attr-escaped.
fn emit_attrs(out: &mut String, attrs: &[(Cow<'_, str>, String)]) {
    for (key, value) in attrs {
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        escape_attr(out, value);
        out.push('"');
    }
}

/// kramdown's `<tr>` emission: one `<th>`/`<td>` per cell, indented two
/// past the row. An empty cell is `<td> </td>` (a single space), and a
/// column's alignment becomes `style="text-align: …"`.
fn emit_table_row(
    out: &mut String,
    ast: &Ast<'_>,
    cells: &[Option<u32>],
    aligns: &[Align],
    head: bool,
    indent: usize,
    hl: &mut dyn CodeHighlighter,
) {
    let tag = if head { "th" } else { "td" };
    push_pad(out, indent);
    out.push_str("<tr>\n");
    for (col, &cell) in cells.iter().enumerate() {
        push_pad(out, indent + 2);
        out.push('<');
        out.push_str(tag);
        match aligns.get(col).copied().unwrap_or(Align::None) {
            Align::None => {}
            Align::Left => out.push_str(" style=\"text-align: left\""),
            Align::Center => out.push_str(" style=\"text-align: center\""),
            Align::Right => out.push_str(" style=\"text-align: right\""),
        }
        out.push('>');
        let before = out.len();
        convert_spans(out, ast, cell, hl.codespan_class());
        if out.len() == before {
            // kramdown fills an empty cell with a non-breaking space.
            out.push('\u{a0}');
        }
        out.push_str("</");
        out.push_str(tag);
        out.push_str(">\n");
    }
    push_pad(out, indent);
    out.push_str("</tr>\n");
}

/// kramdown's list emission. A TIGHT item renders inline:
///   {pad}<li>text</li>
/// or, for a paragraph followed by a nested list, across lines with the child
/// indented one level deeper and `</li>` back at the item's column:
///   {pad}<li>a
///   {pad+2}<ul> … {pad+2}</ul>
///   {pad}</li>
/// A LOOSE item (or a tight item whose content is a non-paragraph block — a
/// pipe-table, code) renders in block form, its content blocks recursively
/// converted at `indent + 4` (paragraphs become `<p>` only when loose).
#[allow(clippy::too_many_arguments)]
fn emit_list(
    out: &mut String,
    ast: &Ast<'_>,
    items: Option<u32>,
    ordered: bool,
    loose: bool,
    ial: &[(Cow<'_, str>, String)],
    indent: usize,
    opts: &Options,
    hl: &mut dyn CodeHighlighter,
    used_ids: &mut HashMap<String, u32>,
) {
    let tag = if ordered { "ol" } else { "ul" };
    push_pad(out, indent);
    out.push('<');
    out.push_str(tag);
    emit_attrs(out, ial);
    out.push_str(">\n");
    let mut cur = items;
    while let Some(idx) = cur {
        let item = &ast.items[idx as usize];
        cur = item.next;
        // A leading paragraph renders inline (`<li>text…`) when the list is
        // tight, or — in a loose list — when this item is "transparent"
        // (kramdown's per-item tight/loose mixing); everything else renders in
        // block form with the paragraph wrapped in `<p>`.
        let para = (!loose || item.transparent)
            .then(|| match item.blocks {
                Some(h) => match &ast.blocks[h as usize].kind {
                    BlockKind::Para(spans) => Some((*spans, ast.blocks[h as usize].next)),
                    _ => None,
                },
                None => None,
            })
            .flatten();
        if let Some((spans, rest)) = para {
            push_pad(out, indent + 2);
            out.push_str("<li>");
            convert_spans(out, ast, spans, hl.codespan_class());
            if rest.is_some() {
                out.push('\n');
                convert_blocks(out, ast, rest, indent + 4, opts, hl, used_ids);
                push_pad(out, indent + 2);
            }
            out.push_str("</li>\n");
            continue;
        }
        // Block form: `<li>` on its own line, content one level deeper.
        push_pad(out, indent + 2);
        if item.blocks.is_none() {
            out.push_str("<li></li>\n"); // empty item
            continue;
        }
        out.push_str("<li>\n");
        convert_blocks(out, ast, item.blocks, indent + 4, opts, hl, used_ids);
        push_pad(out, indent + 2);
        out.push_str("</li>\n");
    }
    push_pad(out, indent);
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

/// A node in the TOC nesting tree: an index into `Ast::toc` plus the tree-arena
/// indices of its child entries.
struct TocTreeNode {
    entry: usize,
    children: Vec<usize>,
}

/// Build the TOC nesting tree from the flat (document-order) entries with
/// kramdown's stack algorithm: an entry nests under the nearest preceding
/// entry of a strictly smaller level, else becomes a root. Returns the tree
/// arena and the root indices.
fn build_toc_tree(entries: &[TocEntry]) -> (Vec<TocTreeNode>, Vec<usize>) {
    let mut tree: Vec<TocTreeNode> = Vec::with_capacity(entries.len());
    let mut roots: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for (ei, e) in entries.iter().enumerate() {
        let ni = tree.len();
        tree.push(TocTreeNode {
            entry: ei,
            children: Vec::new(),
        });
        loop {
            match stack.last().copied() {
                None => {
                    roots.push(ni);
                    stack.push(ni);
                    break;
                }
                Some(top) if entries[tree[top].entry].level < e.level => {
                    tree[top].children.push(ni);
                    stack.push(ni);
                    break;
                }
                Some(_) => {
                    stack.pop();
                }
            }
        }
    }
    (tree, roots)
}

/// kramdown's `{:toc}` table of contents: the marked list is replaced by a
/// `<ul id="markdown-toc">` of links to every heading, nested by level. The
/// list's own IAL becomes the `<ul>` attributes, with `id` defaulting to
/// `markdown-toc` (which also prefixes each entry's own anchor id).
fn emit_toc(
    out: &mut String,
    ast: &Ast<'_>,
    list_ial: &[(Cow<'_, str>, String)],
    ordered: bool,
    indent: usize,
    hl: &mut dyn CodeHighlighter,
) {
    // No collected headings ⇒ kramdown substitutes the empty string, so the
    // list contributes nothing.
    if ast.toc.is_empty() {
        return;
    }
    // The generated list keeps the `{:toc}` list's own type (`<ol>`/`<ul>`) —
    // for the outer list and every nested level.
    let tag = if ordered { "ol" } else { "ul" };
    let mut list_attrs: Vec<(Cow<'_, str>, String)> = list_ial.to_vec();
    let toc_id = match list_attrs.iter().find(|(k, _)| k.as_ref() == "id") {
        Some((_, v)) => v.clone(),
        None => {
            list_attrs.push((Cow::Borrowed("id"), "markdown-toc".to_string()));
            "markdown-toc".to_string()
        }
    };
    let (tree, roots) = build_toc_tree(&ast.toc);
    push_pad(out, indent);
    out.push('<');
    out.push_str(tag);
    emit_attrs(out, &list_attrs);
    out.push_str(">\n");
    emit_toc_nodes(out, ast, &tree, &roots, &toc_id, tag, indent + 2, hl);
    push_pad(out, indent);
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

/// Render a run of sibling TOC nodes as `<li>` items. Each item's leading
/// paragraph is transparent (kramdown), so the link renders inline; a node
/// with children carries a nested `<ul>` on the same line as its link, exactly
/// as kramdown's converter lays it out.
#[allow(clippy::too_many_arguments)]
fn emit_toc_nodes(
    out: &mut String,
    ast: &Ast<'_>,
    tree: &[TocTreeNode],
    node_ids: &[usize],
    toc_id: &str,
    tag: &str,
    li_indent: usize,
    hl: &mut dyn CodeHighlighter,
) {
    for &ni in node_ids {
        let node = &tree[ni];
        let e = &ast.toc[node.entry];
        push_pad(out, li_indent);
        out.push_str("<li><a href=\"#");
        escape_attr(out, &e.id);
        out.push_str("\" id=\"");
        out.push_str(toc_id);
        out.push('-');
        escape_attr(out, &e.id);
        out.push_str("\">");
        convert_spans(out, ast, e.spans, hl.codespan_class());
        out.push_str("</a>");
        if node.children.is_empty() {
            out.push_str("</li>\n");
        } else {
            push_pad(out, li_indent + 2);
            out.push('<');
            out.push_str(tag);
            out.push_str(">\n");
            emit_toc_nodes(out, ast, tree, &node.children, toc_id, tag, li_indent + 4, hl);
            push_pad(out, li_indent + 2);
            out.push_str("</");
            out.push_str(tag);
            out.push_str(">\n");
            push_pad(out, li_indent);
            out.push_str("</li>\n");
        }
    }
}

fn convert_spans(out: &mut String, ast: &Ast<'_>, head: Option<u32>, codespan_class: Option<&str>) {
    let mut cur = head;
    while let Some(idx) = cur {
        let span = &ast.spans[idx as usize];
        cur = span.next;
        match &span.kind {
            SpanKind::Text(t) => escape_text(out, t),
            SpanKind::Raw(html) => out.push_str(html),
            SpanKind::Em(inner) => {
                out.push_str("<em");
                emit_span_ial(out, ast, idx);
                out.push('>');
                convert_spans(out, ast, *inner, codespan_class);
                out.push_str("</em>");
            }
            SpanKind::Strong(inner) => {
                out.push_str("<strong");
                emit_span_ial(out, ast, idx);
                out.push('>');
                convert_spans(out, ast, *inner, codespan_class);
                out.push_str("</strong>");
            }
            SpanKind::Del(inner) => {
                out.push_str("<del>");
                convert_spans(out, ast, *inner, codespan_class);
                out.push_str("</del>");
            }
            SpanKind::Code(code) => {
                match codespan_class {
                    Some(class) => {
                        out.push_str("<code class=\"");
                        out.push_str(class);
                        out.push_str("\">");
                    }
                    None => out.push_str("<code>"),
                }
                escape_text(out, code);
                out.push_str("</code>");
            }
            SpanKind::Link { spans, href, title } => {
                out.push_str("<a href=\"");
                escape_attr(out, href);
                out.push('"');
                if let Some(title) = title {
                    out.push_str(" title=\"");
                    escape_attr(out, title);
                    out.push('"');
                }
                emit_span_ial(out, ast, idx);
                out.push('>');
                convert_spans(out, ast, *spans, codespan_class);
                out.push_str("</a>");
            }
            SpanKind::Image { src, alt, title } => {
                out.push_str("<img src=\"");
                escape_attr(out, src);
                out.push_str("\" alt=\"");
                escape_attr(out, alt);
                out.push('"');
                if let Some(title) = title {
                    out.push_str(" title=\"");
                    escape_attr(out, title);
                    out.push('"');
                }
                emit_span_ial(out, ast, idx);
                out.push_str(" />");
            }
        }
    }
}

/// kramdown `escape_html(…, :text)` — `&`, `<`, `>` only. A SWAR
/// `memchr3` jumps to the next special byte (8 bytes/iter) and the
/// ordinary run before it is bulk-copied; the matched bytes are ASCII,
/// so the slice indices are always char boundaries.
fn escape_text(out: &mut String, text: &str) {
    let bytes = text.as_bytes();
    let mut start = 0;
    while let Some(off) = crate::scan::memchr3(&bytes[start..], b'&', b'<', b'>') {
        let i = start + off;
        if off > 0 {
            out.push_str(&text[start..i]);
        }
        out.push_str(match bytes[i] {
            b'&' => "&amp;",
            b'<' => "&lt;",
            _ => "&gt;",
        });
        start = i + 1;
    }
    if start < bytes.len() {
        out.push_str(&text[start..]);
    }
}

/// kramdown `escape_html(…, :attribute)` — also escapes `"`.
fn escape_attr(out: &mut String, text: &str) {
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        match bytes[start..]
            .iter()
            .position(|&b| matches!(b, b'&' | b'<' | b'>' | b'"'))
        {
            Some(off) => {
                let i = start + off;
                if off > 0 {
                    out.push_str(&text[start..i]);
                }
                out.push_str(match bytes[i] {
                    b'&' => "&amp;",
                    b'<' => "&lt;",
                    b'>' => "&gt;",
                    _ => "&quot;",
                });
                start = i + 1;
            }
            None => {
                out.push_str(&text[start..]);
                break;
            }
        }
    }
}

/// kramdown CORE `Converter::Base#basic_generate_id`: strip leading
/// non-ASCII-letters, delete everything outside `[a-zA-Z0-9 -]`,
/// spaces → hyphens, downcase; empty → "section".
pub(crate) fn basic_generate_id(raw: &str) -> String {
    let stripped = raw.trim_start_matches(|c: char| !c.is_ascii_alphabetic());
    let mut id = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        match ch {
            'a'..='z' | '0'..='9' | '-' => id.push(ch),
            'A'..='Z' => id.push(ch.to_ascii_lowercase()),
            ' ' => id.push('-'),
            _ => {}
        }
    }
    if id.is_empty() {
        id.push_str("section");
    }
    id
}

/// How a single character maps under kramdown-parser-gfm
/// `generate_gfm_header_id`: Unicode downcase, delete `[^\p{Word}\- \t]`,
/// then ` `/`\t` → `-` (one hyphen EACH, no collapsing; leading digits
/// kept, unlike core). See [`slug_char`] for which chars we classify
/// exactly versus decline.
enum SlugChar {
    /// A `\p{Word}` char (or a space → `-`) that lands in the slug.
    Keep(char),
    /// Outside `\p{Word}`, `-`, ` ` — deleted (ASCII punctuation, the
    /// typography codepoints, and the symbol/emoji blocks below).
    Drop,
    /// A char whose `\p{Word}` membership we don't classify exactly (most
    /// non-ASCII letters/marks) — the caller declines rather than risk a
    /// wrong id.
    Unsupported,
}

/// Classify one char for the GFM slug. Shared by [`gfm_slug`] (which
/// builds the slug) and [`gfm_slug_ok`] (the parser's non-allocating
/// validity gate) so the two can never drift.
fn slug_char(ch: char) -> SlugChar {
    use SlugChar::{Drop, Keep, Unsupported};
    match ch {
        'a'..='z' | '0'..='9' | '_' | '-' => Keep(ch),
        'A'..='Z' => Keep(ch.to_ascii_lowercase()),
        ' ' | '\t' => Keep('-'),
        c if c.is_ascii() => Drop, // ASCII punctuation: non-Word
        // Typography output (smart quotes, dashes, ellipsis): non-Word.
        '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' | '\u{2013}' | '\u{2014}'
        | '\u{2026}' => Drop,
        // Symbol/emoji blocks kramdown's `\p{Word}` excludes — deleted like
        // ASCII punctuation. Every entry is Symbol-category (no letters,
        // marks, or digits): Arrows, Misc Technical, Misc Symbols +
        // Dingbats, Misc Symbols & Arrows, and the emoji/pictograph planes.
        // A variation selector / ZWJ riding an emoji is itself a `\p{Word}`
        // mark kramdown keeps; those fall to `Unsupported` below, so an
        // emoji carrying one declines rather than mis-slug.
        '\u{2190}'..='\u{21FF}'
        | '\u{2300}'..='\u{23FF}'
        | '\u{2600}'..='\u{27BF}'
        | '\u{2B00}'..='\u{2BFF}'
        | '\u{1F000}'..='\u{1FAFF}' => Drop,
        // Caseless `\p{Word}` ranges kept verbatim: CJK, kana, hangul.
        c @ ('\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{3040}'..='\u{30FF}'
        | '\u{AC00}'..='\u{D7AF}') => Keep(c),
        _ => Unsupported,
    }
}

pub(crate) fn gfm_slug(span_text: &str) -> Option<String> {
    let mut id = String::with_capacity(span_text.len());
    let bytes = span_text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            // ASCII fast path — heading text is overwhelmingly ASCII, so
            // skip the UTF-8 decode. Mirrors `slug_char`'s ASCII arms
            // exactly: word chars kept (upper → lower), space/tab → `-`,
            // every other ASCII byte (punctuation) dropped.
            match b {
                b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' => id.push(b as char),
                b'A'..=b'Z' => id.push((b + 32) as char),
                b' ' | b'\t' => id.push('-'),
                _ => {}
            }
            i += 1;
        } else {
            // SAFETY-free: index is a char boundary (we only advance past
            // whole chars), so `chars().next()` yields the char at `i`.
            let ch = span_text[i..].chars().next().unwrap();
            match slug_char(ch) {
                SlugChar::Keep(c) => id.push(c),
                SlugChar::Drop => {}
                SlugChar::Unsupported => return None,
            }
            i += ch.len_utf8();
        }
    }
    if id.is_empty() { None } else { Some(id) }
}

/// Whether [`gfm_slug`] would produce `Some` for `span_text`, WITHOUT
/// allocating the slug — the parser only needs the yes/no to decide
/// whether to decline (the converter builds the real slug later). Mirrors
/// `gfm_slug`'s classification exactly via [`slug_char`]: reject the same
/// unsupported chars, and reject an all-deleted (would-be-empty) result.
pub(crate) fn gfm_slug_ok(span_text: &str) -> bool {
    let mut non_empty = false;
    for ch in span_text.chars() {
        match slug_char(ch) {
            SlugChar::Keep(_) => non_empty = true,
            SlugChar::Drop => {}
            SlugChar::Unsupported => return false,
        }
    }
    non_empty
}

/// Duplicate-id suffixing, shared by both algorithms (kramdown core's
/// `@used_ids` and GFM's `@id_counter` behave identically): first use
/// is bare, repeats get `-1`, `-2`, …
pub(crate) fn dedup_id(id: String, used_ids: &mut HashMap<String, u32>) -> String {
    match used_ids.get_mut(&id) {
        Some(count) => {
            *count += 1;
            format!("{id}-{count}")
        }
        None => {
            used_ids.insert(id.clone(), 0);
            id
        }
    }
}

#[cfg(test)]
mod escape_tests {
    //! Pin the SWAR-backed HTML escaping (kramdown escape_html text vs
    //! attribute). A reference scalar escaper is the oracle; we check
    //! leading/trailing/adjacent specials, none-present, and multibyte.
    use super::*;

    fn ref_escape(text: &str, attr: bool) -> String {
        let mut s = String::new();
        for ch in text.chars() {
            match ch {
                '&' => s.push_str("&amp;"),
                '<' => s.push_str("&lt;"),
                '>' => s.push_str("&gt;"),
                '"' if attr => s.push_str("&quot;"),
                c => s.push(c),
            }
        }
        s
    }

    fn run(text: &str) {
        let mut t = String::new();
        escape_text(&mut t, text);
        assert_eq!(t, ref_escape(text, false), "escape_text {text:?}");
        let mut a = String::new();
        escape_attr(&mut a, text);
        assert_eq!(a, ref_escape(text, true), "escape_attr {text:?}");
    }

    #[test]
    fn escaping_matches_reference() {
        for t in [
            "",
            "no specials here",
            "a & b < c > d",
            "&<>",                 // all specials, adjacent
            "<lead",               // leading special
            "trail>",              // trailing special
            "a&&b",                // consecutive same
            "quote \" and & < >",  // quote only escaped in attr
            "café & <naïve>",      // multibyte interspersed
            "&amp; already",       // ampersand of an entity-looking run
            &"x".repeat(100),      // long run, no specials (word-at-a-time path)
        ] {
            run(t);
        }
    }
}
