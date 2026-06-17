//! Hard line breaks. kramdown turns an INTERIOR paragraph line ending in 2+
//! spaces, or a single trailing `\`, into `<br />` (the last two spaces — or
//! the backslash — are consumed; any extra leading spaces are kept). A
//! single trailing space, or trailing whitespace on the LAST line, is not a
//! break. Expected HTML mirrors kramdown 2.5.2 (gem profile).

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
fn two_space_break() {
    ok("line one  \nline two\n", "<p>line one<br />\nline two</p>\n");
    // 3 spaces: the last two become the break, one is kept.
    ok("a   \nb\n", "<p>a <br />\nb</p>\n");
}

#[test]
fn backslash_break() {
    ok("line one\\\nline two\n", "<p>line one<br />\nline two</p>\n");
}

#[test]
fn multiple_and_nested() {
    ok(
        "one  \ntwo  \nthree\n",
        "<p>one<br />\ntwo<br />\nthree</p>\n",
    );
    // A break inside emphasis stays inside it.
    ok("*a  \nb*\n", "<p><em>a<br />\nb</em></p>\n");
}

#[test]
fn not_a_break() {
    // A single trailing space is not a break.
    ok("only one \nb\n", "<p>only one \nb</p>\n");
    // Trailing spaces on the LAST line (paragraph end) are just stripped.
    ok("end  \n\nnext\n", "<p>end</p>\n\n<p>next</p>\n");
}

#[test]
fn corner_declines() {
    // ≥2 trailing backslashes (escaped backslash adjacent to the break) and a
    // trailing tab are kramdown corners — declined, never mis-rendered.
    declined("line \\\\\nnext\n");
    declined("line\t\nnext\n");
}
