//! Span IAL (`…{:.class}`) attachment to the immediately-preceding inline
//! element (link, image, em, strong). Expected HTML mirrors kramdown 2.5.2
//! under the gem profile (`Options::jekyll()`). Chained IALs on the same span
//! (`…{:.a}{:rel=b}`) merge with kramdown's semantics: `class` accumulates,
//! every other key overrides (last wins), insertion order preserved.

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
fn chained_ials_merge() {
    // The real-world shape: `![…](…){:style=…}{:loading=…}` — kramdown merges
    // both IALs onto the element (distinct keys both kept, order preserved).
    ok(
        "![a](/i.png){:style=\"box-shadow: 0 0 1px\"}{:loading=\"lazy\"}\n",
        "<p><img src=\"/i.png\" alt=\"a\" style=\"box-shadow: 0 0 1px\" loading=\"lazy\" /></p>\n",
    );
    // class accumulates (space-joined).
    ok(
        "[t](http://x){:.a}{:.b}\n",
        "<p><a href=\"http://x\" class=\"a b\">t</a></p>\n",
    );
    // id and other keys override (last wins).
    ok(
        "[t](http://x){:#a}{:#b}\n",
        "<p><a href=\"http://x\" id=\"b\">t</a></p>\n",
    );
    ok(
        "[t](http://x){:rel=\"a\"}{:rel=\"b\"}\n",
        "<p><a href=\"http://x\" rel=\"b\">t</a></p>\n",
    );
}
