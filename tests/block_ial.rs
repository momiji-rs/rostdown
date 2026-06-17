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
fn leading_ial_attaches_to_following_block() {
    // kramdown allows a block IAL on EITHER side: `{:.x}` directly BEFORE a
    // block attaches to it (the Jekyll docs "note" idiom puts it first).
    ok(
        "{:.note .info}\nIf you see a warning here.\n",
        "<p class=\"note info\">If you see a warning here.</p>\n",
    );
    ok("{: #custom}\n## Heading\n", "<h2 id=\"custom\">Heading</h2>\n");
    ok(
        "{:.x}\n> quote\n",
        "<blockquote class=\"x\">\n  <p>quote</p>\n</blockquote>\n",
    );
    ok(
        "{:.x}\n- a\n- b\n",
        "<ul class=\"x\">\n  <li>a</li>\n  <li>b</li>\n</ul>\n",
    );
}

#[test]
fn standalone_ial_declines() {
    // No preceding block to attach to.
    declined("{:.x}\n");
    // Separated from its block by a blank — orphaned, out of subset.
    declined("text\n\n{:.x}\n");
    // A leading IAL orphaned by a following blank declines.
    declined("{:.x}\n\ntext\n");
}

#[test]
fn leading_ial_attaches_to_a_fence() {
    // A block IAL directly above a fenced code block lands on `<pre>`; the
    // language class stays on `<code>`.
    ok(
        "{:.x}\n```\ncode\n```\n",
        "<pre class=\"x\"><code>code\n</code></pre>\n",
    );
    ok(
        "{:.minimal-line-height}\n```shell\nls\n```\n",
        "<pre class=\"minimal-line-height\"><code class=\"language-shell\">ls\n</code></pre>\n",
    );
}

#[test]
fn quote_ial_attaches_to_blockquote() {
    // kramdown's "note box" idiom: `{:…}` after a blockquote attaches to the
    // `<blockquote>` itself (NOT the inner paragraph). Multi-line quotes and
    // attribute/id forms behave the same.
    ok(
        "> quoted\n{:.q}\n",
        "<blockquote class=\"q\">\n  <p>quoted</p>\n</blockquote>\n",
    );
    ok(
        "> A note.\n{: .note .info}\n",
        "<blockquote class=\"note info\">\n  <p>A note.</p>\n</blockquote>\n",
    );
    ok(
        "> line one\n> line two\n{:.note}\n",
        "<blockquote class=\"note\">\n  <p>line one\nline two</p>\n</blockquote>\n",
    );
}

#[test]
fn leading_ial_attaches_to_a_table() {
    // A block IAL directly above a pipe-table lands on the `<table>` tag
    // (kramdown attaches a leading IAL to the following block, table included).
    ok(
        "{: .note}\nx | y\n",
        "<table class=\"note\">\n  <tbody>\n    <tr>\n      <td>x</td>\n      <td>y</td>\n    </tr>\n  </tbody>\n</table>\n",
    );
}

#[test]
fn trailing_ial_attaches_to_fence_and_table() {
    // A block IAL directly below a fenced code block lands on `<pre>`.
    ok(
        "```\ncode\n```\n{:.x}\n",
        "<pre class=\"x\"><code>code\n</code></pre>\n",
    );
    // Below a pipe-table it lands on `<table>` (try_parse_table stops at the
    // IAL line, so the rows above form the table).
    ok(
        "a | b\n{:.note}\n",
        "<table class=\"note\">\n  <tbody>\n    <tr>\n      <td>a</td>\n      <td>b</td>\n    </tr>\n  </tbody>\n</table>\n",
    );
}
