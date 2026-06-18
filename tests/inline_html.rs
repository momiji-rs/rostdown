//! Inline raw HTML. kramdown re-serializes inline HTML elements (tag/attr
//! names lowercased, attrs normalized + escaped, void elements ` />`) and
//! parses markdown inside non-void elements. We currently support inline
//! VOID elements (`<br>`, `<img>`, `<wbr>`, …) — emitted verbatim, no
//! content — and decline non-void inline HTML and autolinks. Expected HTML
//! mirrors kramdown 2.5.2 (gem profile).

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
fn inline_void_self_closed() {
    ok("a <br> b\n", "<p>a <br /> b</p>\n");
    ok("a <br/> b\n", "<p>a <br /> b</p>\n");
    ok("a <br /> b\n", "<p>a <br /> b</p>\n");
    ok("line one<br>\nline two\n", "<p>line one<br />\nline two</p>\n");
}

#[test]
fn inline_void_with_attrs() {
    ok(
        "pic <img src=\"x.png\" alt=\"y\"> here\n",
        "<p>pic <img src=\"x.png\" alt=\"y\" /> here</p>\n",
    );
    // bare value quoted, name lowercased.
    ok("pic <IMG SRC=x> end\n", "<p>pic <img src=\"x\" /> end</p>\n");
}

#[test]
fn void_inside_emphasis_and_repeated() {
    ok("*em <br> in*\n", "<p><em>em <br /> in</em></p>\n");
    ok("x <br> y <br> z\n", "<p>x <br /> y <br /> z</p>\n");
}

#[test]
fn non_void_inline_with_markdown_content() {
    // A non-void inline element is re-serialized and its content parsed as
    // markdown (typography included).
    ok(
        "see <a href=\"/x\">link text</a> end\n",
        "<p>see <a href=\"/x\">link text</a> end</p>\n",
    );
    ok(
        "use <abbr title=\"HyperText\">HTML</abbr> ok\n",
        "<p>use <abbr title=\"HyperText\">HTML</abbr> ok</p>\n",
    );
    ok("x <sub>2</sub>O\n", "<p>x <sub>2</sub>O</p>\n");
    ok(
        "a <a href=\"/x\">**bold**</a> b\n",
        "<p>a <a href=\"/x\"><strong>bold</strong></a> b</p>\n",
    );
    ok(
        "q <q>quoted \"x\"</q> e\n",
        "<p>q <q>quoted \u{201c}x\u{201d}</q> e</p>\n",
    );
    // Span-content inline element (`<code>`): markdown NOT parsed, but a
    // well-formed nested tag is re-serialized and a bare `<` is escaped.
    ok(
        "a <code>x &lt; y **lit**</code> b\n",
        "<p>a <code>x &lt; y **lit**</code> b</p>\n",
    );
    ok(
        "a <code><a href=\"u\">y</a></code> b\n",
        "<p>a <code><a href=\"u\">y</a></code> b</p>\n",
    );
    ok(
        "a <samp>p<em>q</em></samp> b\n",
        "<p>a <samp>p<em>q</em></samp> b</p>\n",
    );
}

#[test]
fn autolinks() {
    // kramdown's :autolink — `<scheme:…>` (mailto/http/https/ftp/ftps) and
    // bare `<user@host>` — runs before span HTML, escapes the href + visible
    // text, and `mailto:`-prefixes the bare-email form.
    ok(
        "see <http://example.com> now\n",
        "<p>see <a href=\"http://example.com\">http://example.com</a> now</p>\n",
    );
    ok(
        "q <https://x.io/a?b=1&c=2> e\n",
        "<p>q <a href=\"https://x.io/a?b=1&amp;c=2\">https://x.io/a?b=1&amp;c=2</a> e</p>\n",
    );
    ok(
        "mail <a.b-c@ex_ample.com> me\n",
        "<p>mail <a href=\"mailto:a.b-c@ex_ample.com\">a.b-c@ex_ample.com</a> me</p>\n",
    );
    ok(
        "x <mailto:user@host.org> y\n",
        "<p>x <a href=\"mailto:user@host.org\">user@host.org</a> y</p>\n",
    );
    // An unrecognized scheme / uppercase scheme is NOT an autolink — kramdown
    // keeps it literal (escaped); we decline (safe fallback).
    declined("no <notscheme://x> here\n");
    declined("up <HTTPS://X> here\n");
}

#[test]
fn span_element_at_block_start_is_a_paragraph() {
    // A line opening with a NON-VOID span element is not an HTML block —
    // kramdown starts a paragraph and re-serializes the inline element,
    // parsing markdown inside it.
    ok(
        "<small>by A, B, C</small>\n",
        "<p><small>by A, B, C</small></p>\n",
    );
    ok(
        "<code>:a</code>, <code>:b</code> and more\n",
        "<p><code>:a</code>, <code>:b</code> and more</p>\n",
    );
    ok(
        "<a href=\"/x\">**link**</a> then text\n",
        "<p><a href=\"/x\"><strong>link</strong></a> then text</p>\n",
    );
    // A VOID span element (`<br>`/`<img>`/`<input>`, in kramdown's
    // `HTML_SPAN_ELEMENTS`) at column 0 opens a paragraph too.
    ok("<br>\ntext\n", "<p><br />\ntext</p>\n");
    ok(
        "<img src=\"a.png\" alt=\"x\" />\n",
        "<p><img src=\"a.png\" alt=\"x\" /></p>\n",
    );
    // `<hr>` is a BLOCK-level void element, not a span paragraph — out of
    // subset (a bare `<hr>` is an HR block in kramdown), so we decline.
    declined("<hr>\ntext\n");
    // An unclosed span at block start auto-closes in kramdown; out of subset.
    declined("<em>unclosed\nmore\n");
}

#[test]
fn inline_html_out_of_subset_declines() {
    // Comment / close-without-open.
    declined("a <!-- c --> b\n");
    declined("a </div> b\n");
    // A nested same-name element (no depth tracking — conservative).
    declined("a <span>x <span>y</span> z</span> b\n");
    // An attribute value spanning a line break (kramdown normalizes it).
    declined("Link: <a href=\"test\nfoo\">x</a>\n");
    // A malformed/unclosed tag inside `<code>` — kramdown's recovery (it
    // swallows past the close tag) is out of subset, so decline.
    declined("a <code>x<y & z</code> b\n");
    declined("a <code>vector<int></code> b\n");
}
