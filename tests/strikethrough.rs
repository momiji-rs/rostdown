//! GFM strikethrough `~~text~~` → `<del>text</del>`. Flanking like emphasis:
//! `~~` opens unless followed by a space, closes unless preceded by one; the
//! content is parsed as markdown. A run of 3+ tildes inline is out of subset
//! (declined). Expected HTML mirrors kramdown 2.5.2 (gem profile).

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
fn basic() {
    ok("a ~~struck~~ b\n", "<p>a <del>struck</del> b</p>\n");
    ok("~~x~~\n", "<p><del>x</del></p>\n");
    ok("mid~~word~~end\n", "<p>mid<del>word</del>end</p>\n");
}

#[test]
fn markdown_inside_and_nested() {
    ok("a ~~with *em*~~ b\n", "<p>a <del>with <em>em</em></del> b</p>\n");
    ok("**~~both~~**\n", "<p><strong><del>both</del></strong></p>\n");
    ok(
        "a ~~b~~c~~d~~ e\n",
        "<p>a <del>b</del>c<del>d</del> e</p>\n",
    );
}

#[test]
fn flanking_and_literal_tildes() {
    // Open followed by space / close preceded by space ⇒ literal.
    ok("a ~~ x~~ b\n", "<p>a ~~ x~~ b</p>\n");
    ok("a ~~x ~~ b\n", "<p>a ~~x ~~ b</p>\n");
    // A single tilde is literal.
    ok("plain ~ tilde\n", "<p>plain ~ tilde</p>\n");
    // A trailing extra tilde after the close stays literal.
    ok("a ~~x~~~ b\n", "<p>a <del>x</del>~ b</p>\n");
}

#[test]
fn triple_tilde_inline_declines() {
    declined("~~~x~~~\n");
}
