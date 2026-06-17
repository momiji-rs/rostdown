//! Span IAL (`…{:.class}`) attachment to the immediately-preceding inline
//! element (link, image, em, strong). Expected HTML mirrors kramdown 2.5.2
//! under the gem profile (`Options::jekyll()`). A SINGLE IAL is supported;
//! two abutting IALs on the same span (`…{:.a}{:.b}`) need kramdown's
//! cross-IAL attribute merge (class accumulation, id override) we don't
//! model, so they decline rather than drop the first or duplicate an attr.

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
fn single_ial_on_link() {
    ok(
        "[text](http://x){:.cls}\n",
        "<p><a href=\"http://x\" class=\"cls\">text</a></p>\n",
    );
}

#[test]
fn single_ial_on_image() {
    ok(
        "![a](/i.png){:.cls}\n",
        "<p><img src=\"/i.png\" alt=\"a\" class=\"cls\" /></p>\n",
    );
}

#[test]
fn chained_ials_decline() {
    // The real-world shape: `![…](…){:style=…}{:loading=…}`. kramdown emits
    // both attributes; we'd keep only the last, so decline instead.
    declined("![a](/i.png){:style=\"box-shadow: 0 0 1px\"}{:loading=\"lazy\"}\n");
    declined("[t](http://x){:.a}{:.b}\n");
}
