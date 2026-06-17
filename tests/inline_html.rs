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
fn non_void_inline_and_autolinks_decline() {
    // Non-void inline HTML parses markdown inside (out of the void subset).
    declined("a <span>x</span> b\n");
    declined("see <a href=\"/x\">link</a>\n");
    declined("use <abbr title=\"HyperText\">HTML</abbr>\n");
    // Autolinks.
    declined("see <http://example.com> now\n");
    // Comment / close-without-open.
    declined("a <!-- c --> b\n");
    declined("a </div> b\n");
}
