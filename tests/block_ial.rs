//! Block IAL (`{:.class}` / `{:#id}` …) attachment to the preceding
//! paragraph, heading, or list. Expected HTML is byte-for-byte what
//! kramdown 2.5.2 produces under the gem's profile (`Options::jekyll()` ⇒
//! GFM input, `auto_ids: true`), so each `ok` case is a literal mirror of
//! a `Kramdown::Document.new(src, input: "GFM", auto_ids: true).to_html`
//! run. The `declined` cases pin the frontier: orphan/standalone IALs and
//! span IALs stay out of subset and fall back to Ruby kramdown.

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

// ---- Paragraph ------------------------------------------------------------

#[test]
fn para_class() {
    ok("text\n{:.note}\n", "<p class=\"note\">text</p>\n");
}

#[test]
fn para_id() {
    ok("text\n{:#x}\n", "<p id=\"x\">text</p>\n");
}

// ---- Heading: IAL composes with the auto-id -------------------------------

#[test]
fn heading_class_keeps_auto_id() {
    // The IAL class lands first; auto_ids still slugs the title into `id`.
    ok(
        "## Title\n{:.h}\n",
        "<h2 class=\"h\" id=\"title\">Title</h2>\n",
    );
}

#[test]
fn heading_id_replaces_auto_id() {
    // An IAL-supplied id wins: kramdown emits only that id, no auto-id.
    ok("## Title\n{:#myid}\n", "<h2 id=\"myid\">Title</h2>\n");
}

#[test]
fn heading_class_and_id() {
    // Both from the IAL, insertion order preserved, no auto-id.
    ok(
        "## Title\n{:.c #myid}\n",
        "<h2 class=\"c\" id=\"myid\">Title</h2>\n",
    );
}

// ---- List: IAL lands on the opening tag -----------------------------------

#[test]
fn ul_class() {
    ok(
        "- a\n- b\n{:.x}\n",
        "<ul class=\"x\">\n  <li>a</li>\n  <li>b</li>\n</ul>\n",
    );
}

#[test]
fn ol_class() {
    ok("1. a\n{:.x}\n", "<ol class=\"x\">\n  <li>a</li>\n</ol>\n");
}

#[test]
fn ul_id() {
    ok("- a\n{:#lid}\n", "<ul id=\"lid\">\n  <li>a</li>\n</ul>\n");
}

#[test]
fn ial_between_items_splits_the_list() {
    // A `{:.x}` abutting the first item terminates the list (attaching to
    // it); the next marker opens a fresh, attribute-less list — exactly
    // kramdown's split.
    ok(
        "- a\n{:.x}\n- b\n",
        "<ul class=\"x\">\n  <li>a</li>\n</ul>\n<ul>\n  <li>b</li>\n</ul>\n",
    );
}

// ---- Frontier: still declined ---------------------------------------------

#[test]
fn standalone_ial_declines() {
    // No preceding block to attach to.
    declined("{:.x}\n");
    // Separated from its block by a blank — orphaned, out of subset.
    declined("text\n\n{:.x}\n");
}
