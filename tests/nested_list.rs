//! Tight nested-list golden cases — the construct behind the
//! jekyll-1k corpus's 457-document decline (every post carries a
//! `- a\n  - nested` list). Expected HTML is byte-for-byte what
//! kramdown 2.5.2 (GFM input, auto_ids off) produces — probed on
//! CRuby; see the probe table in the task notes. The official
//! kramdown corpus only exercises loose/multi-paragraph lists
//! (still declined), so these pin the tight-nesting subset.

use rostdown::{Error, NoHighlight, Options, to_html};

fn render(src: &str) -> Result<String, Error> {
    to_html(src, &Options::jekyll(), &mut NoHighlight)
}

#[track_caller]
fn ok(src: &str, expected: &str) {
    match render(src) {
        Ok(html) => assert_eq!(html, expected, "input: {src:?}"),
        Err(e) => panic!("declined {src:?}: {e:?}"),
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
fn nested_two_levels() {
    ok(
        "- a\n  - b\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b</li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn nested_with_tail_item() {
    ok(
        "- a\n  - b\n- c\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b</li>\n    </ul>\n  </li>\n  <li>c</li>\n</ul>\n",
    );
}

#[test]
fn nested_three_levels() {
    ok(
        "- a\n  - b\n    - c\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b\n        <ul>\n          <li>c</li>\n        </ul>\n      </li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn nested_mid_list() {
    ok(
        "- a\n- b\n  - c\n- d\n",
        "<ul>\n  <li>a</li>\n  <li>b\n    <ul>\n      <li>c</li>\n    </ul>\n  </li>\n  <li>d</li>\n</ul>\n",
    );
}

#[test]
fn ordered_child_in_unordered() {
    ok(
        "- a\n  1. b\n",
        "<ul>\n  <li>a\n    <ol>\n      <li>b</li>\n    </ol>\n  </li>\n</ul>\n",
    );
}

#[test]
fn two_children() {
    ok(
        "- a\n  - x\n  - y\n- b\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>x</li>\n      <li>y</li>\n    </ul>\n  </li>\n  <li>b</li>\n</ul>\n",
    );
}

#[test]
fn continuation_then_child() {
    ok(
        "- a\n  cont\n  - b\n",
        "<ul>\n  <li>a\ncont\n    <ul>\n      <li>b</li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn child_then_deep_continuation() {
    // The 2-space continuation AFTER a child marker attaches to the
    // deepest open item (kramdown joins it onto `b`).
    ok(
        "- a\n  - b\n  cont\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b\ncont</li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn corpus_post_shape() {
    // The exact list every jekyll-1k post carries.
    ok(
        "- item one 7\n- item two\n  - nested\n",
        "<ul>\n  <li>item one 7</li>\n  <li>item two\n    <ul>\n      <li>nested</li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn ordered_parent_nested_child() {
    // An ordered parent carries an indented child at its digits+2 content
    // column (kramdown nests it just like an unordered parent).
    ok(
        "1. a\n   1. b\n",
        "<ol>\n  <li>a\n    <ol>\n      <li>b</li>\n    </ol>\n  </li>\n</ol>\n",
    );
}

#[test]
fn conservative_declines_hold() {
    // A TAB-indented child is now nested (kramdown expands the tab to 4 spaces).
    ok(
        "- a\n\t- b\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b</li>\n    </ul>\n  </li>\n</ul>\n",
    );
    // 1-space indent is a SAME-level item in kramdown — declined.
    declined("- a\n - b\n");
}

#[test]
fn loose_nesting_and_lazy_join_to_deepest_item() {
    // Blank-separated nesting is LOOSE — the parent's text wraps in `<p>` and
    // the child list follows after a blank line.
    ok(
        "- a\n\n  - b\n",
        "<ul>\n  <li>\n    <p>a</p>\n\n    <ul>\n      <li>b</li>\n    </ul>\n  </li>\n</ul>\n",
    );
    // A column-0 lazy line after a nested child joins the DEEPEST open item
    // (kramdown's aggressive lazy continuation), not the parent or a new block.
    ok(
        "- a\n  - b\ncont\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b\ncont</li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn tight_item_with_block_content() {
    // A tight item that is a paragraph followed by a block (code / heading /
    // blockquote, absorbed as a column-0 lazy block) renders inline-then-block,
    // exactly like a paragraph + nested list.
    ok(
        "1. first\n2. text:\n```sh\ncode\n```\n",
        "<ol>\n  <li>first</li>\n  <li>text:\n    <pre><code class=\"language-sh\">code\n</code></pre>\n  </li>\n</ol>\n",
    );
    ok(
        "- a\n> quote\n",
        "<ul>\n  <li>a\n    <blockquote>\n      <p>quote</p>\n    </blockquote>\n  </li>\n</ul>\n",
    );
}

#[test]
fn ordered_item_with_shallow_indent_nested_ul() {
    // A nested list indented less than the ordered marker's content column
    // (`1. ` = column 3, nested `*` at 2) is a lazy continuation kramdown
    // re-parses as an OPT_SPACE nested list.
    ok(
        "1. text:\n  * a\n  * b\n",
        "<ol>\n  <li>text:\n    <ul>\n      <li>a</li>\n      <li>b</li>\n    </ul>\n  </li>\n</ol>\n",
    );
    // A same-kind shallow marker is a same-level sibling, not a nest — out of
    // subset, declines.
    declined("- a\n - b\n");
    // Mixed shallow indents (kramdown splits into separate lists) — declines.
    declined("1. a\n  * x\n   * y\n");
}
