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
fn inline_html_out_of_subset_declines() {
    // Autolinks.
    declined("see <http://example.com> now\n");
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
