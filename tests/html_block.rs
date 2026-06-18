//! Raw HTML blocks, re-serialized to match kramdown's GFM-profile HTML
//! converter (`Options::jekyll()`): tag/attr names lowercased, attribute
//! values re-quoted with `"` and HTML-attribute-escaped, attribute order
//! preserved, boolean attributes `name=""`, void elements ` />`, text
//! HTML-escaped with entities kept verbatim, markdown NOT parsed inside.
//! Expected HTML is a literal mirror of kramdown 2.5.2. Out-of-subset
//! blocks (tables, comments, `markdown=`, raw-text elements) decline.

use rostdown::{Error, NoHighlight, Options, to_html};

fn render(src: &str) -> Result<String, Error> {
    to_html(src, &Options::jekyll(), &mut NoHighlight)
}

#[track_caller]
fn ok(src: &str, expected: &str) {
    match render(src) {
        Ok(html) => assert_eq!(html, expected, "input: {src:?}"),
        Err(e) => panic!("expected byte-identical render, declined {src:?}: {e:?}"),
    }
}

#[track_caller]
fn declined(src: &str) {
    match render(src) {
        Ok(html) => panic!("expected decline for {src:?}, got {html:?}"),
        Err(Error::Declined(_)) => {}
    }
}

#[test]
fn note_div_keeps_markdown_literal() {
    // Markdown inside a block HTML element is NOT parsed (`**bold**` stays).
    ok(
        "<div class=\"note\">\nText with **bold**.\n</div>\n",
        "<div class=\"note\">\nText with **bold**.\n</div>\n",
    );
}

#[test]
fn trailing_space_in_tag_normalized() {
    ok(
        "<div class=\"videoWrapper\" >\n<iframe src=\"x\"></iframe>\n</div>\n",
        "<div class=\"videoWrapper\">\n<iframe src=\"x\"></iframe>\n</div>\n",
    );
}

#[test]
fn void_element_self_closed() {
    ok(
        "<figure>\n  <img src=\"a.png\" alt=\"x\">\n</figure>\n",
        "<figure>\n  <img src=\"a.png\" alt=\"x\" />\n</figure>\n",
    );
}

#[test]
fn names_lowercased_attrs_normalized() {
    // Tag/attr names lowercase (values kept), bare value quoted, boolean
    // written `name=""`.
    ok(
        "<DIV CLASS=\"X\" data-y=z hidden>up</DIV>\n",
        "<div class=\"X\" data-y=\"z\" hidden=\"\">up</div>\n",
    );
}

#[test]
fn text_and_code_nested_tags() {
    // Text nodes escape `<`/`>`/bare-`&`; entities stay verbatim. A nested
    // `<code>` is span-content: a WELL-FORMED nested tag is re-serialized
    // (not escaped), while a bare `<` becomes `&lt;` and markdown stays
    // literal.
    ok(
        "<div><code><a href=\"u\">y</a></code> &amp; &copy;</div>\n",
        "<div><code><a href=\"u\">y</a></code> &amp; &copy;</div>\n",
    );
    ok(
        "<div><code>a &lt; b</code> **lit**</div>\n",
        "<div><code>a &lt; b</code> **lit**</div>\n",
    );
}

#[test]
fn block_interrupts_paragraph() {
    ok(
        "x\n<div>y</div>\nz\n",
        "<p>x</p>\n<div>y</div>\n<p>z</p>\n",
    );
}

#[test]
fn surrounded_by_paragraphs() {
    ok(
        "before\n\n<div class=\"note\">\nhi\n</div>\n\nafter\n",
        "<p>before</p>\n\n<div class=\"note\">\nhi\n</div>\n\n<p>after</p>\n",
    );
}

#[test]
fn custom_and_unknown_block_elements() {
    // kramdown treats an unknown / custom element at a block boundary as a
    // `:block`-content element (content verbatim, markdown not parsed,
    // nested tags re-serialized) — like a div. Common in modern Bridgetown
    // (web components).
    ok(
        "<is-land on:visible import=\"x\">\n  <breezy-day></breezy-day>\n</is-land>\n",
        "<is-land on:visible=\"\" import=\"x\">\n  <breezy-day></breezy-day>\n</is-land>\n",
    );
    ok(
        "<my-widget>\nThis has **markdown** inside.\n</my-widget>\n",
        "<my-widget>\nThis has **markdown** inside.\n</my-widget>\n",
    );
}

#[test]
fn table_family_reserialized() {
    // kramdown re-serializes a raw `<table>` through the ordinary block path
    // (names lowercased, void children ` />`, bare attrs quoted, text
    // verbatim, markdown NOT parsed) — like a div. The common Jekyll-docs
    // shape is a `<table>` wrapped in a scroller `<div>`.
    ok(
        "<table class=\"t\">\n<tr><td>**a**</td><td></td></tr>\n</table>\n",
        "<table class=\"t\">\n<tr><td>**a**</td><td></td></tr>\n</table>\n",
    );
    ok(
        "<TABLE>\n<TR><TD CLASS=x>cell<BR></TD></TR>\n</TABLE>\n",
        "<table>\n<tr><td class=\"x\">cell<br /></td></tr>\n</table>\n",
    );
    ok(
        "<div class=\"mobile-side-scroller\">\n<table>\n  <thead>\n    <tr><th>Setting</th></tr>\n  </thead>\n</table>\n</div>\n",
        "<div class=\"mobile-side-scroller\">\n<table>\n  <thead>\n    <tr><th>Setting</th></tr>\n  </thead>\n</table>\n</div>\n",
    );
}

#[test]
fn out_of_subset_declines() {
    declined("<div markdown=\"1\">\n**b**\n</div>\n"); // markdown= changes parsing
    declined("<pre>\n  code\n</pre>\n"); // pre: bespoke whitespace rules
    declined("<div>unclosed\n"); // no close tag
    declined("<div>a</div> trailing\n"); // content after close on same line
}

#[test]
fn raw_text_script_and_style_render_verbatim() {
    // `<script>`/`<style>` content is kept verbatim — no markdown, no escaping
    // of `<`/`>`/`&` — and kramdown always trails the block with a blank line.
    ok(
        "<script>\nif (a < b && c > d) f();\n</script>\n",
        "<script>\nif (a < b && c > d) f();\n</script>\n\n",
    );
    ok(
        "<style>\n.a > .b { color: red }\n</style>\n",
        "<style>\n.a > .b { color: red }\n</style>\n\n",
    );
    // A following source blank line is absorbed (not emitted twice).
    ok(
        "<style>\n.x {}\n</style>\n\ntext\n",
        "<style>\n.x {}\n</style>\n\n<p>text</p>\n",
    );
    // An unclosed raw-text element declines.
    declined("<script>\nvar x = 1;\n");
}

#[test]
fn html_comment_blocks_render_verbatim() {
    // A comment block is kept verbatim (no markdown, no escaping) up to the
    // first `-->`; surrounding paragraphs are unaffected.
    ok("<!-- a comment -->\n", "<!-- a comment -->\n");
    ok(
        "<!--\n## not a heading\nverbatim\n-->\n",
        "<!--\n## not a heading\nverbatim\n-->\n",
    );
    ok(
        "text\n\n<!-- note -->\n\nmore\n",
        "<p>text</p>\n\n<!-- note -->\n\n<p>more</p>\n",
    );
    // Content after the closing `-->` on the same line is out of subset.
    declined("<!-- c --> trailing\n");
}

#[test]
fn leading_block_ial_attaches_to_html_block() {
    // A block IAL on the line above an HTML block injects its attributes into
    // the block's root tag (kramdown attaches the IAL to the element).
    ok(
        "{: style=\"text-align: center\"}\n<p>hi</p>\n",
        "<p style=\"text-align: center\">hi</p>\n",
    );
    ok("{:.note}\n<div>x</div>\n", "<div class=\"note\">x</div>\n");
    // Merging an IAL onto an element that already has attributes (class
    // accumulation / key override) is out of subset — decline.
    declined("{:.b}\n<div class=\"a\">x</div>\n");
}

#[test]
fn multi_line_span_element_opens_a_paragraph() {
    // A span-level element (`<em>`, `<small>`, …) at column 0 is NOT an HTML
    // block: it opens a paragraph whose content — including the closing tag on
    // its own line — is parsed as markdown spans and wrapped in `<p>`. The
    // closing-tag line must not be mistaken for a new HTML block.
    ok(
        "<em>\nsome *text* and [a](/u)\n</em>\n",
        "<p><em>\nsome <em>text</em> and <a href=\"/u\">a</a>\n</em></p>\n",
    );
    ok(
        "<small>\nfine print [here](/x)\n</small>\n",
        "<p><small>\nfine print <a href=\"/x\">here</a>\n</small></p>\n",
    );
}
