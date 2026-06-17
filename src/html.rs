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

use crate::parse::{Align, Ast, BlockKind, SpanKind};
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
            } => {
                emit_list(out, ast, *items, *ordered, *loose, &block.ial, indent, hl);
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
                    out.push_str("<pre><code");
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
            BlockKind::RawHtml(html) => {
                // Already serialized to match kramdown's HTML converter
                // (parsed at column 0, top level) — emit verbatim.
                out.push_str(html);
                out.push('\n');
            }
            BlockKind::Table {
                aligns,
                header,
                body,
            } => {
                push_pad(out, indent);
                out.push_str("<table>\n");
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
        }
    }
}

/// Emit a span element's IAL attributes (`[t](u){:.c}`), if any, from the
/// side table — used inside the open tag, after the element's own attrs.
#[inline]
fn emit_span_ial(out: &mut String, ast: &Ast<'_>, idx: u32) {
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

/// kramdown's tight-list emission: leaf items are single-line
/// (`{pad}<li>text</li>`); an item with a nested child opens
/// across lines with the child <ul>/<ol> indented one level
/// deeper and `</li>` back at the item's own column:
///   {pad}<li>a
///   {pad+2}<ul>
///   {pad+4}<li>b</li>
///   {pad+2}</ul>
///   {pad}</li>
#[allow(clippy::too_many_arguments)]
fn emit_list(
    out: &mut String,
    ast: &Ast<'_>,
    items: Option<u32>,
    ordered: bool,
    loose: bool,
    ial: &[(Cow<'_, str>, String)],
    indent: usize,
    hl: &mut dyn CodeHighlighter,
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
        if loose {
            // kramdown wraps each loose item's content in a `<p>`; loose
            // lists in the v1 subset never carry a nested child.
            push_pad(out, indent + 2);
            out.push_str("<li>\n");
            push_pad(out, indent + 4);
            out.push_str("<p>");
            convert_spans(out, ast, item.spans, hl.codespan_class());
            out.push_str("</p>\n");
            push_pad(out, indent + 2);
            out.push_str("</li>\n");
            continue;
        }
        push_pad(out, indent + 2);
        out.push_str("<li>");
        convert_spans(out, ast, item.spans, hl.codespan_class());
        match &item.child {
            Some((c_ord, c_items)) => {
                out.push('\n');
                emit_list(out, ast, *c_items, *c_ord, false, &[], indent + 4, hl);
                push_pad(out, indent + 2);
                out.push_str("</li>\n");
            }
            None => out.push_str("</li>\n"),
        }
    }
    push_pad(out, indent);
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
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
fn basic_generate_id(raw: &str) -> String {
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
    for ch in span_text.chars() {
        match slug_char(ch) {
            SlugChar::Keep(c) => id.push(c),
            SlugChar::Drop => {}
            SlugChar::Unsupported => return None,
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
fn dedup_id(id: String, used_ids: &mut HashMap<String, u32>) -> String {
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
