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
//! at column 0 with a block-level container element and close at a line
//! boundary; raw-text elements with non-text escaping (`script`, `style`,
//! `pre`, `table` family, …), comments, doctypes, processing instructions,
//! a `markdown=` attribute, and any malformed / unclosed / mismatched
//! structure all bail to `None`.

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

/// Raw-content elements whose body is read verbatim to the close tag and
/// then HTML-escaped exactly like ordinary text (`<code>a<b</code>` →
/// `<code>a&lt;b</code>`) — no nested-tag parsing.
fn is_escaped_raw(name: &str) -> bool {
    matches!(name, "code" | "kbd" | "samp" | "var")
}

/// Raw-content elements kramdown serializes WITHOUT escaping (`<script>`,
/// `<style>`, `<math>`) or with bespoke rules (`pre`, `textarea`, `table`
/// family). Out of subset — decline.
fn is_decline_raw(name: &str) -> bool {
    matches!(
        name,
        "script"
            | "style"
            | "math"
            | "textarea"
            | "title"
            | "option"
            | "pre"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "td"
            | "th"
            | "caption"
            | "colgroup"
    )
}

/// Whether `name` starts a top-level HTML BLOCK — a known block element
/// (`div`, `figure`, `section`, `p`, `ul`, …) OR an unknown/custom element
/// (`is-land`, `my-widget`, `sl-button`). kramdown treats both as a
/// `:block`-content element at a block boundary (content verbatim, markdown
/// not parsed, nested tags re-serialized) — exactly what [`serialize`]
/// produces. EXCLUDED: known span/void elements (`<span>`/`<br>` at column 0
/// are `<p>`-wrapped, out of subset) and raw-text elements (`<table>`,
/// `<script>`, …, with a different content model).
fn is_block_start(name: &str) -> bool {
    !is_decline_raw(name) && !is_inline(name)
}

/// Whether `line` (already known to be at a block boundary) begins a
/// supported HTML block: column 0, `<name` with `name` a block-start
/// element. Used by the paragraph scanner to break before an HTML block and
/// by the block loop to dispatch into [`serialize`].
pub(crate) fn starts_html_block(line: &str) -> bool {
    let b = line.as_bytes();
    if b.first() != Some(&b'<') {
        return false;
    }
    let mut j = 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
        j += 1;
    }
    if j == 1 {
        return false;
    }
    // Name must be followed by whitespace, `>`, or `/` (end of the tag name).
    if !matches!(b.get(j), None | Some(b' ') | Some(b'\t') | Some(b'>') | Some(b'/')) {
        return false;
    }
    let name = line[1..j].to_ascii_lowercase();
    is_block_start(&name)
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

        if is_escaped_raw(&name) {
            // Body is verbatim up to the matching close tag, then escaped.
            let close = format!("</{name}>");
            let rest = &self.s[self.pos..];
            let end = find_close_ci(rest, &name)?;
            Self::push_escaped_text(out, &rest[..end]);
            self.pos += end + close.len();
            out.push_str(&close);
            return Some(());
        }

        // Normal content: text runs and nested elements until our close tag.
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

/// Find the matching `</name>` (case-insensitive) in `s`, returning the byte
/// offset of the `<`. Used only for escaped-raw elements (no nesting).
fn find_close_ci(s: &str, name: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == b'<' && b[i + 1] == b'/' {
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
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
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
    /// Raw-content element (`<code>`/`<kbd>`/…): emit `open`, the HTML-escaped
    /// `body`, then `close`, all verbatim (no markdown inside).
    Raw {
        open: String,
        body: String,
        close: String,
    },
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

    if is_escaped_raw(&name) {
        let mut body = String::new();
        Parser::push_escaped_text(&mut body, content);
        return Some((Inline::Raw { open, body, close }, after_close));
    }
    Some((Inline::Markdown { open, content, close }, after_close))
}

/// Serialize one top-level HTML block at the start of `src` (which begins at
/// column 0 with a block-start tag). Returns the serialized HTML and the
/// number of bytes consumed, requiring the close tag to land at a line
/// boundary (end of input or a `\n`). `None` ⇒ out of subset → decline.
pub(crate) fn serialize(src: &str) -> Option<(String, usize)> {
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
    Some((out, p.pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert!(is_void("img") && is_void("br") && is_void("hr"));
        assert!(!is_void("div") && !is_void("span"));
        assert!(is_escaped_raw("code") && !is_escaped_raw("div"));
        assert!(is_decline_raw("script") && is_decline_raw("table") && is_decline_raw("pre"));
        assert!(is_block_start("div") && is_block_start("figure") && is_block_start("p"));
        assert!(!is_block_start("span") && !is_block_start("a") && !is_block_start("table"));
    }

    #[test]
    fn starts_detection() {
        assert!(starts_html_block("<div class=\"x\">"));
        assert!(starts_html_block("<figure>"));
        assert!(starts_html_block("<DIV>")); // case-insensitive
        assert!(!starts_html_block("<span>x</span>")); // span not a block start
        assert!(!starts_html_block("<table>")); // raw → not a start
        assert!(!starts_html_block("<!-- c -->")); // comment
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
    }

    #[test]
    fn serialize_declines() {
        assert_eq!(ser("<!-- c -->"), None); // comment
        assert_eq!(ser("<table><tr><td>a</td></tr></table>"), None); // raw family
        assert_eq!(ser("<div markdown=\"1\">x</div>"), None); // markdown attr
        assert_eq!(ser("<div>unclosed"), None); // no close
        assert_eq!(ser("<div>a</div> trailing"), None); // trailing content
        assert_eq!(ser("<div>a</span>"), None); // mismatched close
        assert_eq!(ser("<script>x</script>"), None); // raw no-escape
    }
}
