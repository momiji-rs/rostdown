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
fn text_and_raw_code_escaping() {
    // Text nodes escape `<`/`>`/bare-`&`; entities stay verbatim; a nested
    // `<code>` (raw) escapes its body the same way.
    ok(
        "<div><code>a<b & c</code> &amp; &copy;</div>\n",
        "<div><code>a&lt;b &amp; c</code> &amp; &copy;</div>\n",
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
fn out_of_subset_declines() {
    declined("<table class=\"x\">\n<tr><td>a</td></tr>\n</table>\n"); // table family is :raw
    declined("<!-- a comment -->\n"); // comment
    declined("<div markdown=\"1\">\n**b**\n</div>\n"); // markdown= changes parsing
    declined("<script>var x=1;</script>\n"); // raw, no-escape content
    declined("<pre>\n  code\n</pre>\n"); // pre raw content
    declined("<div>unclosed\n"); // no close tag
    declined("<div>a</div> trailing\n"); // content after close on same line
}
