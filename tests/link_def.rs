//! Block link reference definitions (`[id]: dest "title"`). Mirrors
//! kramdown's `LINK_DEFINITION_START` + its post-check: the bare
//! destination may contain spaces (so an unexpanded Liquid `{{ … }}` URL
//! resolves) but not whitespace-then-quote; `(…)` is NOT a title in a
//! definition; `[^id]:` is a footnote, not a link def. Expected HTML mirrors
//! kramdown 2.5.2 (gem profile, NoHighlight).

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
fn liquid_url_resolves() {
    // The real-world case: an unexpanded Liquid URL (spaces, no quote) is a
    // valid destination — both engines resolve the reference.
    ok(
        "See [the issue][1].\n\n[1]: {{ site.repo }}/issues/9\n",
        "<p>See <a href=\"{{ site.repo }}/issues/9\">the issue</a>.</p>\n\n",
    );
}

#[test]
fn destination_with_title() {
    ok(
        "[x][a]\n\n[a]: http://e.com \"Home\"\n",
        "<p><a href=\"http://e.com\" title=\"Home\">x</a></p>\n\n",
    );
}

#[test]
fn spaced_destination_no_title() {
    ok(
        "[x][a]\n\n[a]: one two three\n",
        "<p><a href=\"one two three\">x</a></p>\n\n",
    );
}

#[test]
fn parens_stay_in_destination() {
    // `(…)` is a title only in inline links, never in a definition.
    ok(
        "[x][a]\n\n[a]: dest (note)\n",
        "<p><a href=\"dest (note)\">x</a></p>\n\n",
    );
}

#[test]
fn footnote_definition_is_not_a_link_def() {
    // `[^id]:` is a footnote (out of subset) — must decline, not be swallowed
    // as a link definition (which would drop the footnote markup).
    declined("text[^a]\n\n[^a]: a footnote\n");
}

#[test]
fn whitespace_quote_in_destination_declines() {
    // kramdown's post-check rejects a destination containing whitespace+
    // quote; we decline (the doc still renders correctly via fallback).
    declined("[x][a]\n\n[a]: a b \"t\" junk\n");
}
