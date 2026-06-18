//! Raw HTML block re-serialization, matching kramdown's default
//! (`parse_block_html: false`) HTML parser + `Converter::Html`. kramdown
//! does NOT pass an HTML block through verbatim — it parses it into a tree
//! and re-serializes: tag/attr names lowercased, attribute values
//! re-quoted with `"` and HTML-attribute-escaped, attribute order
//! preserved, boolean attributes written `name=""`, void elements closed
//! ` />`, and text content HTML-escaped (`<`/`>`/bare `&`) with recognized
//! entities kept verbatim. Markdown inside is NOT parsed.
//!
//! We reproduce a conservative subset and DECLINE the rest (so the document
//! falls back to Ruby kramdown, never renders wrong): the block must start
//! at column 0 with a block-level container element (including the `table`
//! family and unknown/custom elements) and close at a line boundary. The
//! verbatim raw-text elements `script`/`style` are supported (content kept
//! exactly, no escaping, a trailing blank line as kramdown emits); the
//! raw-content elements with bespoke whitespace/escaping rules (`pre`,
//! `textarea`, `title`, `option`, `math`), comments, doctypes, processing
//! instructions, a `markdown=` attribute, and any malformed / unclosed /
//! mismatched structure all bail to `None`.

/// kramdown `HTML_ELEMENTS_WITHOUT_BODY` — serialized ` />`, no close tag.
fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "command"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "keygen"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Span-content elements (`code`/`kbd`/`samp`/`var`) whose body kramdown
/// parses for nested HTML but NOT markdown: a well-formed nested tag is
/// re-serialized (`<code><a href="…">x</a></code>` kept), text is escaped,
/// and a `**bold**` stays literal. [`element`] produces exactly this, so they
/// flow through the ordinary element path — this predicate only marks them as
/// inline (for `is_inline`) and selects the no-markdown inline route.
fn is_escaped_raw(name: &str) -> bool {
    matches!(name, "code" | "kbd" | "samp" | "var")
}

/// Raw-text elements whose content kramdown keeps VERBATIM — no markdown, no
/// nested-HTML parsing, no escaping (`<`/`>`/`&` pass through unchanged) — until
/// the matching close tag. We reproduce `script`/`style` exactly this way; the
/// opening tag's attributes still re-serialize normally.
fn is_raw_text(name: &str) -> bool {
    matches!(name, "script" | "style")
}

/// Raw-content elements with bespoke whitespace / escaping rules we do NOT
/// reproduce (`math`'s MathML model; `pre`/`textarea`'s leading-newline strip;
/// `title`/`option`). Out of subset — decline.
///
/// The table family (`table`/`thead`/`tbody`/`tfoot`/`tr`/`td`/`th`/`caption`/
/// `colgroup`) is NOT here: kramdown re-serializes those through the ordinary
/// block-element tree path (names lowercased, void children ` />`, text
/// verbatim, markdown not parsed) — exactly what [`serialize`] produces — so
/// they are in subset.
fn is_decline_raw(name: &str) -> bool {
    matches!(name, "math" | "textarea" | "title" | "option" | "pre")
}

/// Whether `name` starts a top-level HTML BLOCK — a known block element
/// (`div`, `figure`, `section`, `p`, `ul`, …) OR an unknown/custom element
/// (`is-land`, `my-widget`, `sl-button`). kramdown treats both as a
/// `:block`-content element at a block boundary (content verbatim, markdown
/// not parsed, nested tags re-serialized) — exactly what [`serialize`]
/// produces. Verbatim raw-text elements (`script`/`style`) are block starts
/// too — [`element`] gives them the raw-content path. EXCLUDED: known
/// span/void elements (`<span>`/`<br>` at column 0 are `<p>`-wrapped, out of
/// subset) and the bespoke raw-content elements ([`is_decline_raw`]).
fn is_block_start(name: &str) -> bool {
    !is_decline_raw(name) && !is_inline(name)
}

/// If `line` begins with a tag-name token `<name` (terminated by whitespace,
/// `>`, or `/`), return the lowercased `name`; else `None`.
fn leading_tag_name(line: &str) -> Option<String> {
    let b = line.as_bytes();
    if b.first() != Some(&b'<') {
        return None;
    }
    let mut j = 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
        j += 1;
    }
    if j == 1 {
        return None;
    }
    if !matches!(b.get(j), None | Some(b' ') | Some(b'\t') | Some(b'>') | Some(b'/')) {
        return None;
    }
    Some(line[1..j].to_ascii_lowercase())
}

/// Whether `line` (already known to be at a block boundary) begins a
/// supported HTML block: column 0, `<name` with `name` a block-start
/// element. Used by the paragraph scanner to break before an HTML block and
/// by the block loop to dispatch into [`serialize`].
pub(crate) fn starts_html_block(line: &str) -> bool {
    line.starts_with("<!--") || leading_tag_name(line).is_some_and(|n| is_block_start(&n))
}

/// Whether `line` begins with a NON-VOID span-level HTML element — `<em>`,
/// `<a>`, `<code>`, `<small>`, `<sub>`, … kramdown does NOT open an HTML block
/// on these: the line starts an ordinary paragraph whose span parser
/// re-serializes the inline element (markdown parsed inside), so the block
/// scanner must NOT decline it as a raw HTML block. Void elements are
/// excluded: some (`<hr>`) are block-level in kramdown (a bare `<hr>` is an
/// HR block, not a `<p>`-wrapped span), so declining them stays safe.
pub(crate) fn starts_span_element(line: &str) -> bool {
    leading_tag_name(line).is_some_and(|n| {
        // Non-void inline elements, plus the VOID elements kramdown classifies
        // as span-level (`br`/`img`/`input` — in `HTML_SPAN_ELEMENTS`, so they
        // open a `<p>` at column 0; `hr`/`link`/`meta`/… are block-level void
        // and stay out of this paragraph path).
        (is_inline(&n) && !is_void(&n)) || matches!(n.as_str(), "br" | "img" | "input")
    })
}

/// Whether `line` begins with the CLOSING tag of a span-level element
/// (`</em>`, `</a>`, `</small>`, …). When a span element opens a paragraph
/// across several lines, its closing tag lands at the start of a continuation
/// line — that line is inline content of the paragraph, not a new HTML block,
/// so the paragraph scanner must NOT decline it.
pub(crate) fn starts_span_element_close(line: &str) -> bool {
    let b = line.as_bytes();
    if b.first() != Some(&b'<') || b.get(1) != Some(&b'/') {
        return false;
    }
    let mut j = 2;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
        j += 1;
    }
    if j == 2 {
        return false;
    }
    let name = line[2..j].to_ascii_lowercase();
    is_inline(&name) && !is_void(&name)
}

struct Parser<'a> {
    b: &'a [u8],
    s: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    /// Append `text[..]` HTML-escaped (`<`/`>`/bare-`&`), keeping a
    /// recognized entity reference verbatim — kramdown's text-node output.
    fn push_escaped_text(out: &mut String, text: &str) {
        let tb = text.as_bytes();
        let mut k = 0;
        while k < tb.len() {
            match tb[k] {
                b'<' => {
                    out.push_str("&lt;");
                    k += 1;
                }
                b'>' => {
                    out.push_str("&gt;");
                    k += 1;
                }
                b'&' => {
                    // A recognized entity stays verbatim; a bare `&` escapes.
                    if let Some((_, len)) = crate::parse::parse_entity_at(&text[k..]) {
                        out.push_str(&text[k..k + len]);
                        k += len;
                    } else {
                        out.push_str("&amp;");
                        k += 1;
                    }
                }
                _ => {
                    // Copy a run of ordinary bytes (UTF-8 safe: the matched
                    // bytes above are all ASCII).
                    let start = k;
                    while k < tb.len() && !matches!(tb[k], b'<' | b'>' | b'&') {
                        k += 1;
                    }
                    out.push_str(&text[start..k]);
                }
            }
        }
    }

    /// Append `value` HTML-attribute-escaped (`&`/`<`/`>`/`"`), entities
    /// kept verbatim — kramdown's `escape_html(:attribute)`.
    fn push_escaped_attr(out: &mut String, value: &str) {
        let vb = value.as_bytes();
        let mut k = 0;
        while k < vb.len() {
            match vb[k] {
                b'<' => {
                    out.push_str("&lt;");
                    k += 1;
                }
                b'>' => {
                    out.push_str("&gt;");
                    k += 1;
                }
                b'"' => {
                    out.push_str("&quot;");
                    k += 1;
                }
                b'&' => {
                    if let Some((_, len)) = crate::parse::parse_entity_at(&value[k..]) {
                        out.push_str(&value[k..k + len]);
                        k += len;
                    } else {
                        out.push_str("&amp;");
                        k += 1;
                    }
                }
                _ => {
                    let start = k;
                    while k < vb.len() && !matches!(vb[k], b'<' | b'>' | b'"' | b'&') {
                        k += 1;
                    }
                    out.push_str(&value[start..k]);
                }
            }
        }
    }

    /// Parse and serialize one element at `pos` (`b[pos] == b'<'`, not a
    /// close tag). `None` ⇒ out of subset.
    fn element(&mut self, out: &mut String) -> Option<()> {
        debug_assert_eq!(self.peek(), Some(b'<'));
        self.pos += 1;
        // Comments / doctype / PI / CDATA / close-without-open: out of subset.
        if !self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        let name_start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            self.pos += 1;
        }
        let name = self.s[name_start..self.pos].to_ascii_lowercase();
        if is_decline_raw(&name) {
            return None;
        }

        out.push('<');
        out.push_str(&name);
        let self_closed = self.attributes(out)?;

        if is_void(&name) || self_closed {
            out.push_str(" />");
            return Some(());
        }
        out.push('>');

        if is_raw_text(&name) {
            // Raw-text element (`script`/`style`): content is verbatim — no
            // markdown, no nested-HTML parsing, no escaping — until the
            // matching close tag (the first `</name>`, case-insensitive).
            let content_start = self.pos;
            loop {
                while self.peek().is_some_and(|c| c != b'<') {
                    self.pos += 1;
                }
                self.peek()?; // EOF before the close tag ⇒ unclosed, decline
                if self.b.get(self.pos + 1) == Some(&b'/') {
                    let after = &self.s[self.pos + 2..];
                    if let Some(nend) = after
                        .bytes()
                        .position(|c| !(c.is_ascii_alphanumeric() || c == b'-'))
                        && after[..nend].eq_ignore_ascii_case(&name)
                    {
                        let mut p = self.pos + 2 + nend;
                        while self.b.get(p).is_some_and(|c| matches!(c, b' ' | b'\t')) {
                            p += 1;
                        }
                        if self.b.get(p) == Some(&b'>') {
                            out.push_str(&self.s[content_start..self.pos]);
                            out.push_str("</");
                            out.push_str(&name);
                            out.push('>');
                            self.pos = p + 1;
                            return Some(());
                        }
                    }
                }
                self.pos += 1; // this `<` is raw content; keep scanning
            }
        }

        // Normal content: text runs and nested elements until our close tag.
        // This includes `code`/`kbd`/`samp`/`var`: in block context kramdown
        // gives them a `:span` content model, so a well-formed nested tag
        // (`<code><a href="…">x</a></code>`) is re-serialized as an element,
        // NOT escaped; only a bare/malformed `<` becomes `&lt;`.
        loop {
            match self.peek()? {
                b'<' => match self.b.get(self.pos + 1) {
                    Some(b'/') => {
                        // A close tag — must be ours.
                        let after = &self.s[self.pos + 2..];
                        let nend = after
                            .bytes()
                            .position(|c| !(c.is_ascii_alphanumeric() || c == b'-'))?;
                        if !after[..nend].eq_ignore_ascii_case(&name) {
                            return None; // mismatched close
                        }
                        // Allow only whitespace before the `>`.
                        let mut p = self.pos + 2 + nend;
                        while self.b.get(p).is_some_and(|c| matches!(c, b' ' | b'\t')) {
                            p += 1;
                        }
                        if self.b.get(p) != Some(&b'>') {
                            return None;
                        }
                        out.push_str("</");
                        out.push_str(&name);
                        out.push('>');
                        self.pos = p + 1;
                        return Some(());
                    }
                    // `<` + letter starts a nested element; a malformed tag
                    // there declines (kramdown's recovery is out of subset).
                    Some(c) if c.is_ascii_alphabetic() => self.element(out)?,
                    // A bare `<` not starting a tag (`a < b`, `<<`, `< 3`) is
                    // literal text — escape it, matching kramdown.
                    _ => {
                        out.push_str("&lt;");
                        self.pos += 1;
                    }
                },
                _ => {
                    let start = self.pos;
                    while self.peek().is_some_and(|c| c != b'<') {
                        self.pos += 1;
                    }
                    Self::push_escaped_text(out, &self.s[start..self.pos]);
                }
            }
        }
    }

    /// Serialize the attribute list, returning whether the tag self-closed
    /// (`/>`). Leaves `pos` just past the `>`.
    fn attributes(&mut self, out: &mut String) -> Option<bool> {
        loop {
            self.skip_ws();
            match self.peek()? {
                b'>' => {
                    self.pos += 1;
                    return Some(false);
                }
                b'/' => {
                    if self.b.get(self.pos + 1) == Some(&b'>') {
                        self.pos += 2;
                        return Some(true);
                    }
                    return None;
                }
                c if c.is_ascii_alphabetic() || c == b'_' || c == b':' => {
                    let ns = self.pos;
                    while self.peek().is_some_and(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b':' | b'.')
                    }) {
                        self.pos += 1;
                    }
                    let aname = self.s[ns..self.pos].to_ascii_lowercase();
                    // `markdown=` changes content parsing entirely — decline.
                    if aname == "markdown" {
                        return None;
                    }
                    self.skip_ws();
                    out.push(' ');
                    out.push_str(&aname);
                    out.push_str("=\"");
                    if self.peek() == Some(b'=') {
                        self.pos += 1;
                        self.skip_ws();
                        let value = self.attr_value()?;
                        // A newline inside an attribute value is normalized
                        // to a space by kramdown (`href="a\nb"` → `"a b"`);
                        // we don't reproduce that, so decline.
                        if value.contains('\n') {
                            return None;
                        }
                        Self::push_escaped_attr(out, value);
                    }
                    out.push('"');
                }
                _ => return None,
            }
        }
    }

    fn attr_value(&mut self) -> Option<&'a str> {
        match self.peek()? {
            q @ (b'"' | b'\'') => {
                self.pos += 1;
                let start = self.pos;
                while self.peek().is_some_and(|c| c != q) {
                    self.pos += 1;
                }
                if self.peek() != Some(q) {
                    return None; // unterminated
                }
                let v = &self.s[start..self.pos];
                self.pos += 1;
                Some(v)
            }
            _ => {
                // Bare value: up to whitespace or `>`.
                let start = self.pos;
                while self
                    .peek()
                    .is_some_and(|c| !matches!(c, b' ' | b'\t' | b'\n' | b'>' | b'/'))
                {
                    self.pos += 1;
                }
                if self.pos == start {
                    return None;
                }
                Some(&self.s[start..self.pos])
            }
        }
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n')) {
            self.pos += 1;
        }
    }
}

/// Inline (span-level) HTML elements kramdown re-serializes in span context
/// while parsing markdown inside them.
fn is_inline(name: &str) -> bool {
    is_void(name)
        || is_escaped_raw(name)
        || matches!(
            name,
            "a" | "abbr"
                | "acronym"
                | "b"
                | "bdo"
                | "big"
                | "button"
                | "cite"
                | "del"
                | "dfn"
                | "em"
                | "i"
                | "ins"
                | "label"
                | "mark"
                | "q"
                | "rb"
                | "rbc"
                | "rp"
                | "rt"
                | "rtc"
                | "ruby"
                | "s"
                | "small"
                | "span"
                | "strike"
                | "strong"
                | "sub"
                | "sup"
                | "time"
                | "tt"
                | "u"
        )
}

/// One inline HTML element parsed at a `<` in span context.
pub(crate) enum Inline<'a> {
    /// Void element (`<br>` → `<br />`): emit `html` verbatim.
    Void(String),
    /// Span-content element with markdown NOT parsed inside
    /// (`code`/`kbd`/`samp`/`var`): the whole element is already serialized
    /// (nested tags kept, text escaped) — emit `html` verbatim.
    Raw(String),
    /// Normal element: emit `open`, then the markdown-parsed `content`, then
    /// `close`.
    Markdown {
        open: String,
        content: &'a str,
        close: String,
    },
}

/// Find the matching `</name>` for an inline element whose content starts at
/// `start`, returning `(content_end, after_close)` byte offsets. Declines
/// (`None`) on a nested same-name open tag (no depth tracking — conservative)
/// or a missing close.
fn find_inline_close(s: &str, start: usize, name: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let mut i = start;
    while i + 1 < b.len() {
        if b[i] == b'<' {
            if b[i + 1] == b'/' {
                let after = &s[i + 2..];
                let nend = after
                    .bytes()
                    .position(|c| !(c.is_ascii_alphanumeric() || c == b'-'))?;
                if after[..nend].eq_ignore_ascii_case(name) {
                    let mut p = i + 2 + nend;
                    while b.get(p).is_some_and(|c| matches!(c, b' ' | b'\t')) {
                        p += 1;
                    }
                    if b.get(p) == Some(&b'>') {
                        return Some((i, p + 1));
                    }
                }
            } else if b[i + 1].is_ascii_alphabetic() {
                let after = &s[i + 1..];
                let nend = after
                    .bytes()
                    .position(|c| !(c.is_ascii_alphanumeric() || c == b'-'))
                    .unwrap_or(after.len());
                if after[..nend].eq_ignore_ascii_case(name) {
                    return None; // nested same-name element — out of subset
                }
            }
        }
        i += 1;
    }
    None
}

/// Parse one inline HTML element at the start of `s` (`s[0] == '<'`, next a
/// letter), returning the parsed element and bytes consumed. `None` if it
/// isn't a clean inline element (block-level name, `markdown=`, malformed,
/// unclosed, self-closed non-void, nested same-name).
pub(crate) fn inline_at(s: &str) -> Option<(Inline<'_>, usize)> {
    let mut p = Parser {
        b: s.as_bytes(),
        s,
        pos: 0,
    };
    if p.peek() != Some(b'<') {
        return None;
    }
    p.pos += 1;
    if !p.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let name_start = p.pos;
    while p
        .peek()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        p.pos += 1;
    }
    let name = s[name_start..p.pos].to_ascii_lowercase();
    if !is_inline(&name) {
        return None; // block-level / unknown inline element — out of subset
    }
    if is_escaped_raw(&name) {
        // code/kbd/samp/var: span content model — nested HTML elements are
        // kept (re-serialized), text is escaped, and markdown is NOT parsed.
        // element() produces exactly that and declines on kramdown's quirky
        // unclosed-/mismatched-tag recovery (out of subset).
        let mut whole = String::with_capacity(s.len());
        let mut ep = Parser {
            b: s.as_bytes(),
            s,
            pos: 0,
        };
        ep.element(&mut whole)?;
        return Some((Inline::Raw(whole), ep.pos));
    }
    let mut open = String::with_capacity(name.len() + 8);
    open.push('<');
    open.push_str(&name);
    let self_closed = p.attributes(&mut open)?;

    if is_void(&name) {
        open.push_str(" />");
        return Some((Inline::Void(open), p.pos));
    }
    if self_closed {
        return None; // self-closed non-void inline element — rare, decline
    }
    open.push('>');
    let close = format!("</{name}>");
    let content_start = p.pos;
    let (content_end, after_close) = find_inline_close(s, content_start, &name)?;
    let content = &s[content_start..content_end];
    Some((Inline::Markdown { open, content, close }, after_close))
}

/// Serialize one top-level HTML block at the start of `src` (which begins at
/// column 0 with a block-start tag). Returns the serialized HTML and the
/// number of bytes consumed, requiring the close tag to land at a line
/// boundary (end of input or a `\n`). `None` ⇒ out of subset → decline.
pub(crate) fn serialize(src: &str) -> Option<(String, usize)> {
    // An HTML comment block: kramdown keeps the content verbatim (no markdown,
    // no escaping) up to the first `-->`, which must end at a line boundary.
    if let Some(rest) = src.strip_prefix("<!--") {
        let end = rest.find("-->")?;
        let close = "<!--".len() + end + "-->".len();
        if src[close..].starts_with(|c| c != '\n') {
            return None; // trailing content on the close line is out of subset
        }
        return Some((src[..close].to_string(), close));
    }
    // Must open with a block-start element.
    if !starts_html_block(src) {
        return None;
    }
    let mut p = Parser {
        b: src.as_bytes(),
        s: src,
        pos: 0,
    };
    let mut out = String::with_capacity(src.len() + 16);
    p.element(&mut out)?;
    // The element must end exactly at a line boundary; trailing content on
    // the same line is out of subset.
    if p.pos != src.len() && src.as_bytes().get(p.pos) != Some(&b'\n') {
        return None;
    }
    // A raw-text block (`script`/`style`) is always followed by a blank line
    // in kramdown's output, regardless of the source. Bake the extra newline
    // in and absorb one following source blank line so it isn't emitted twice.
    if leading_tag_name(src).is_some_and(|n| is_raw_text(&n)) {
        out.push('\n');
        let bytes = src.as_bytes();
        if bytes.get(p.pos) == Some(&b'\n') {
            let next_start = p.pos + 1;
            let next_end = src[next_start..]
                .find('\n')
                .map_or(src.len(), |x| next_start + x);
            if src[next_start..next_end].trim().is_empty() {
                p.pos = next_start; // consume the trailing blank line
            }
        }
    }
    Some((out, p.pos))
}

/// kramdown autolink at the start of `s` (`s[0] == '<'`): `<scheme:…>` with
/// scheme ∈ `mailto`/`http`/`https`/`ftp`/`ftps`, or `<user@host>` with both
/// sides drawn from `[[:alnum:]]-_.`. Returns the serialized
/// `<a href="…">text</a>` and the byte length consumed (through the `>`).
/// Mirrors `Kramdown::Parser::Kramdown::AUTOLINK_START` + `parse_autolink`:
/// the href is `mailto:`-prefixed for the bare-email form, and the visible
/// text drops a leading `mailto:`. `.`/ACHARS never match a newline, so the
/// inner text may not cross a line.
pub(crate) fn autolink_at(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'<') {
        return None;
    }
    let mut gt = 1;
    while gt < b.len() && b[gt] != b'>' && b[gt] != b'\n' {
        gt += 1;
    }
    if gt >= b.len() || b[gt] != b'>' || gt == 1 {
        return None; // no closing `>` on this line, or empty `<>`
    }
    let inner = &s[1..gt];
    let consumed = gt + 1;

    // Scheme branch (`(mailto|https?|ftps?):` then `.+`). Case-sensitive, like
    // the regex (no `i` flag). Check `https`/`ftps` before their prefixes.
    let is_scheme = ["mailto:", "https:", "http:", "ftps:", "ftp:"]
        .into_iter()
        .any(|p| inner.strip_prefix(p).is_some_and(|rest| !rest.is_empty()));

    let (href, text): (String, &str) = if is_scheme {
        let text = inner.strip_prefix("mailto:").unwrap_or(inner);
        (inner.to_string(), text)
    } else if is_autolink_email(inner) {
        (format!("mailto:{inner}"), inner)
    } else {
        return None;
    };

    let mut out = String::with_capacity(inner.len() + 24);
    out.push_str("<a href=\"");
    Parser::push_escaped_attr(&mut out, &href);
    out.push_str("\">");
    Parser::push_escaped_text(&mut out, text);
    out.push_str("</a>");
    Some((out, consumed))
}

/// `[[:alnum:]]-_.]+@[[:alnum:]]-_.]+` over ASCII: exactly one `@`, non-empty
/// on both sides, every other byte an ASCII alnum / `-` / `_` / `.`. A
/// non-ASCII char fails (kramdown's Unicode `[[:alnum:]]` is out of subset —
/// decline rather than risk a near-miss).
fn is_autolink_email(inner: &str) -> bool {
    let Some(at) = inner.find('@') else {
        return false;
    };
    let (user, host) = (&inner[..at], &inner[at + 1..]);
    if user.is_empty() || host.is_empty() {
        return false;
    }
    let achar = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.');
    user.bytes().all(achar) && host.bytes().all(achar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert!(is_void("img") && is_void("br") && is_void("hr"));
        assert!(!is_void("div") && !is_void("span"));
        assert!(is_escaped_raw("code") && !is_escaped_raw("div"));
        assert!(is_raw_text("script") && is_raw_text("style") && !is_raw_text("div"));
        assert!(is_decline_raw("pre") && is_decline_raw("math") && !is_decline_raw("table"));
        assert!(!is_decline_raw("script") && !is_decline_raw("style"));
        // raw-text elements are block starts (re-serialized verbatim).
        assert!(is_block_start("script") && is_block_start("style"));
        assert!(is_block_start("div") && is_block_start("figure") && is_block_start("p"));
        // The table family is now a block start (re-serialized like a div).
        assert!(is_block_start("table") && is_block_start("td") && is_block_start("tr"));
        assert!(!is_block_start("span") && !is_block_start("a") && !is_block_start("code"));
    }

    #[test]
    fn starts_detection() {
        assert!(starts_html_block("<div class=\"x\">"));
        assert!(starts_html_block("<figure>"));
        assert!(starts_html_block("<DIV>")); // case-insensitive
        assert!(starts_html_block("<table>")); // table family re-serialized
        assert!(!starts_html_block("<span>x</span>")); // span not a block start
        assert!(!starts_html_block("<code>x</code>")); // code is span/inline
        assert!(starts_html_block("<!-- c -->")); // comment is a block
        assert!(!starts_html_block("not a tag"));
        assert!(!starts_html_block("<")); // bare
        assert!(!starts_html_block(" <div>")); // leading space → handled elsewhere
    }

    fn ser(s: &str) -> Option<String> {
        serialize(s).map(|(h, _)| h)
    }

    #[test]
    fn serialize_cases() {
        assert_eq!(ser("<div>x</div>\n").as_deref(), Some("<div>x</div>"));
        // names lowercased, value kept, bare quoted, boolean ="".
        assert_eq!(
            ser("<DIV A=B c hidden>x</DIV>").as_deref(),
            Some("<div a=\"B\" c=\"\" hidden=\"\">x</div>")
        );
        // void → " />", bare `<`/`&` escaped, nested tag + entity verbatim.
        assert_eq!(
            ser("<div>a < b & <br> <b>x</b> &copy;</div>").as_deref(),
            Some("<div>a &lt; b &amp; <br /> <b>x</b> &copy;</div>")
        );
        // attribute value escaping.
        assert_eq!(
            ser("<div title='a\"b<c&d'>x</div>").as_deref(),
            Some("<div title=\"a&quot;b&lt;c&amp;d\">x</div>")
        );
        // table family re-serialized like a div: names lowercased, void child
        // ` />`, text verbatim (markdown not parsed).
        assert_eq!(
            ser("<TABLE>\n<TR><TD CLASS=x>**a**<BR></TD></TR>\n</TABLE>\n").as_deref(),
            Some("<table>\n<tr><td class=\"x\">**a**<br /></td></tr>\n</table>")
        );
        // code in block context: well-formed nested tag kept (NOT escaped).
        assert_eq!(
            ser("<div><code><a href=\"u\">y</a></code></div>").as_deref(),
            Some("<div><code><a href=\"u\">y</a></code></div>")
        );
        // raw-text (`script`/`style`): content verbatim (no escaping of
        // `<`/`>`/`&`), attributes re-serialized, a trailing blank-line newline
        // baked in (kramdown always trails a raw-text block with a blank line).
        assert_eq!(
            ser("<script>if (a < b && c > d) {}</script>").as_deref(),
            Some("<script>if (a < b && c > d) {}</script>\n")
        );
        assert_eq!(
            ser("<style>\n.x { color: red }\n</style>").as_deref(),
            Some("<style>\n.x { color: red }\n</style>\n")
        );
        assert_eq!(
            ser("<script type=\"text/javascript\">var x=1;</script>").as_deref(),
            Some("<script type=\"text/javascript\">var x=1;</script>\n")
        );
    }

    #[test]
    fn serialize_comments() {
        // Comment content kept verbatim up to the first `-->`.
        assert_eq!(ser("<!-- c -->").as_deref(), Some("<!-- c -->"));
        assert_eq!(
            ser("<!--\nmulti\nline\n-->").as_deref(),
            Some("<!--\nmulti\nline\n-->")
        );
        assert_eq!(ser("<!---\n## x\n-->").as_deref(), Some("<!---\n## x\n-->"));
        assert_eq!(ser("<!-- unterminated"), None); // no closing -->
        assert_eq!(ser("<!-- c --> trailing"), None); // content after close
    }

    #[test]
    fn serialize_declines() {
        assert_eq!(ser("<div markdown=\"1\">x</div>"), None); // markdown attr
        assert_eq!(ser("<div>unclosed"), None); // no close
        assert_eq!(ser("<div>a</div> trailing"), None); // trailing content
        assert_eq!(ser("<div>a</span>"), None); // mismatched close
        assert_eq!(ser("<script>x"), None); // unclosed raw-text
        assert_eq!(ser("<pre>x</pre>"), None); // pre: bespoke whitespace, declined
    }
}
