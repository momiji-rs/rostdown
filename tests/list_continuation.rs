//! List-continuation coverage — the construct behind the top real-content
//! decline (`list-continuation`) once tables landed. Expected HTML is
//! byte-for-byte what kramdown 2.5.2 produces under the gem's profile
//! (`Options::jekyll()` ⇒ GFM input, `auto_ids: false`, `hard_wrap:
//! false` — a newline inside an item joins with `\n`, not `<br />`).
//!
//! The `ok` cases pin the continuation shapes rostdown already renders
//! identically (regression guard). The `declines_*` cases document the
//! frontier: each is a continuation kramdown accepts that rostdown
//! conservatively declines today, with the kramdown target noted so the
//! case flips `declined → ok` when support lands. Right-or-declined holds
//! throughout — a continuation is never rendered wrong.

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

// ---- Supported: byte-identical to kramdown -------------------------------

#[test]
fn unordered_two_space_continuation() {
    ok(
        "- first line\n  second line\n",
        "<ul>\n  <li>first line\nsecond line</li>\n</ul>\n",
    );
}

#[test]
fn single_line_item_trailing_whitespace_is_trimmed() {
    // On a single-line item kramdown trims trailing whitespace — even 2+
    // spaces make no `<br />` (nothing follows to break to). A multi-line
    // item where the break would land still declines (see
    // `declines_irregular_indentation`).
    ok("* a \n* b\n", "<ul>\n  <li>a</li>\n  <li>b</li>\n</ul>\n");
    ok("* a  \n* b\n", "<ul>\n  <li>a</li>\n  <li>b</li>\n</ul>\n");
    ok(
        "1. one  \n2. two\t\n",
        "<ol>\n  <li>one</li>\n  <li>two</li>\n</ol>\n",
    );
}

#[test]
fn unordered_lazy_column0_continuation() {
    ok(
        "- first\nsecond\n",
        "<ul>\n  <li>first\nsecond</li>\n</ul>\n",
    );
}

#[test]
fn unordered_multiple_continuation_lines() {
    ok("- a\n  b\n  c\n", "<ul>\n  <li>a\nb\nc</li>\n</ul>\n");
}

#[test]
fn continuation_keeps_inline_markup() {
    ok(
        "- see `x`\n  and *y*\n",
        "<ul>\n  <li>see <code>x</code>\nand <em>y</em></li>\n</ul>\n",
    );
}

#[test]
fn ordered_tight_single_line() {
    ok(
        "1. a\n2. b\n3. c\n",
        "<ol>\n  <li>a</li>\n  <li>b</li>\n  <li>c</li>\n</ol>\n",
    );
}

#[test]
fn ordered_lazy_column0_continuation() {
    ok(
        "1. first\nsecond\n",
        "<ol>\n  <li>first\nsecond</li>\n</ol>\n",
    );
}

#[test]
fn ordered_indented_continuation() {
    // The common numbered-list-with-wrapped-text shape: the continuation
    // is indented to the marker's content column (digits + ". ").
    ok(
        "1. first line\n   second line\n2. next\n",
        "<ol>\n  <li>first line\nsecond line</li>\n  <li>next</li>\n</ol>\n",
    );
}

// ---- Frontier: kramdown accepts, rostdown declines (never wrong) ----------

#[test]
fn loose_lists() {
    // A blank line between every adjacent pair ⇒ a uniformly loose list:
    // each item's content wraps in `<p>`.
    ok(
        "- a\n\n- b\n",
        "<ul>\n  <li>\n    <p>a</p>\n  </li>\n  <li>\n    <p>b</p>\n  </li>\n</ul>\n",
    );
    ok(
        "1. a\n\n2. b\n",
        "<ol>\n  <li>\n    <p>a</p>\n  </li>\n  <li>\n    <p>b</p>\n  </li>\n</ol>\n",
    );
}

#[test]
fn indented_opt_space_lists() {
    // kramdown ignores a 1-3-space OPT_SPACE base indent on a list.
    ok(
        " * a\n * b\n",
        "<ul>\n  <li>a</li>\n  <li>b</li>\n</ul>\n",
    );
    ok(
        "  1. x\n  2. y\n",
        "<ol>\n  <li>x</li>\n  <li>y</li>\n</ol>\n",
    );
    // A lazy continuation in an opt-space list keeps its FULL leading
    // whitespace verbatim (kramdown does not strip a lazy line), so a 1-space
    // residual survives — the base-aware parse reproduces it exactly.
    ok(
        " - item one\n as part two\n",
        "<ul>\n  <li>item one\n as part two</li>\n</ul>\n",
    );
    ok(
        " - item one\n   indented cont\n",
        "<ul>\n  <li>item one\nindented cont</li>\n</ul>\n",
    );
}

#[test]
fn declines_mixed_and_multiblock_lists() {
    // Mixing abutting and blank-separated items renders per-item in
    // kramdown (some `<li>x</li>`, some `<li><p>x</p></li>`) — out of subset.
    declined("- a\n- b\n\n- c\n");
    // A multi-paragraph item (blank then an indented block) is out of subset.
    declined("- a\n\n  more\n- b\n");
}

#[test]
fn opt_space_list_interrupts_paragraph() {
    // GFM: an OPT_SPACE-indented list start (`  - …`) directly after a
    // paragraph line interrupts the paragraph (the list isn't swallowed as
    // continuation text).
    ok(
        "intro text:\n  - item one\n  - item two\n",
        "<p>intro text:</p>\n<ul>\n  <li>item one</li>\n  <li>item two</li>\n</ul>\n",
    );
}

#[test]
fn emphasis_spanning_a_continuation() {
    // kramdown joins an item's lines, then parses inline — so `*` opening on
    // one physical line pairs with `*` on the next. The recursive item parse
    // now joins the lines before span parsing, matching that exactly.
    ok(
        "- a *open\n  close* b\n",
        "<ul>\n  <li>a <em>open\nclose</em> b</li>\n</ul>\n",
    );
    ok(
        "- a **strong\n  across** b\n",
        "<ul>\n  <li>a <strong>strong\nacross</strong> b</li>\n</ul>\n",
    );
    // Bare marker on the marker line, closer on the continuation.
    ok(
        "- start *here\n  there*\n",
        "<ul>\n  <li>start <em>here\nthere</em></li>\n</ul>\n",
    );
    // An inline link whose brackets straddle the line break.
    ok(
        "- see [the\n  docs](http://x)\n",
        "<ul>\n  <li>see <a href=\"http://x\">the\ndocs</a></li>\n</ul>\n",
    );
    ok(
        "1. accurately [set the timezone\n   codes](/win)\n",
        "<ol>\n  <li>accurately <a href=\"/win\">set the timezone\ncodes</a></li>\n</ol>\n",
    );
}

#[test]
fn balanced_brackets_in_continuation_still_ok() {
    // Balanced brackets that don't span the join stay literal in both
    // engines, so the item still renders (no over-decline).
    ok(
        "- see arr[0] for\n  more info\n",
        "<ul>\n  <li>see arr[0] for\nmore info</li>\n</ul>\n",
    );
}

#[test]
fn declines_irregular_indentation() {
    // A 1-space-indented MARKER is a same-level sibling kramdown keeps but we
    // don't model (off the list's own indent).
    declined("- a\n - b\n");
    // Tab indentation.
    declined("- a\n\tb\n");
}

#[test]
fn shallow_lazy_continuation_keeps_its_indent() {
    // A 1-space-indented PLAIN continuation is a lazy line; kramdown keeps its
    // full leading space (it is not a content line, so not stripped).
    ok("- a\n b\n", "<ul>\n  <li>a\n b</li>\n</ul>\n");
}

#[test]
fn trailing_whitespace_across_a_continuation_is_a_hard_break() {
    // 2+ trailing spaces on a non-last item line break to the next line — the
    // recursive item parse runs the paragraph hard-break logic, matching
    // kramdown's `<br />`.
    ok(
        "- a  \n  b\n",
        "<ul>\n  <li>a<br />\nb</li>\n</ul>\n",
    );
}

#[test]
fn loose_list_uniform_multiblock_renders() {
    // A loose list whose items are all paragraph-only (one multi-paragraph
    // item among single-paragraph siblings, every item blank-separated)
    // renders uniformly — every item's paragraphs wrapped in `<p>`.
    ok(
        "- a\n\n  more\n\n- b\n",
        "<ul>\n  <li>\n    <p>a</p>\n\n    <p>more</p>\n  </li>\n  <li>\n    <p>b</p>\n  </li>\n</ul>\n",
    );
}

#[test]
fn loose_list_with_non_paragraph_item_declines() {
    // A loose list that ALSO has an item carrying a non-paragraph block (code,
    // nested list) triggers kramdown's per-item tight/loose mixing — out of
    // subset, declines.
    declined("1. a\n\n2. b\n   ```\n   x\n   ```\n");
    declined("1. a\n\n2. b\n   * x\n");
}
