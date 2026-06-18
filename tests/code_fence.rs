//! Fenced code blocks with varying fence lengths. kramdown opens on a run
//! of ≥3 backticks/tildes and closes on a run of the SAME char at least as
//! long (no info string) — so a longer fence closes a shorter one, and a
//! shorter run inside a longer fence is literal content. Expected HTML
//! mirrors kramdown 2.5.2 under the gem profile (`Options::jekyll()`,
//! NoHighlight).

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
fn three_closes_three() {
    ok(
        "```sh\ncode line\n```\n",
        "<pre><code class=\"language-sh\">code line\n</code></pre>\n",
    );
}

#[test]
fn four_backtick_line_closes_a_three_fence() {
    // The real-world shape: a 3-fence accidentally closed with 4 backticks.
    ok("```\na\n````\n", "<pre><code>a\n</code></pre>\n");
}

#[test]
fn four_open_four_close() {
    ok(
        "````sh\nb\n````\n",
        "<pre><code class=\"language-sh\">b\n</code></pre>\n",
    );
}

#[test]
fn shorter_run_inside_longer_fence_is_content() {
    // A bare ``` (3) and a ```-prefixed line do NOT close a 4-fence.
    ok(
        "````\ninner ```\nstill code\n````\n",
        "<pre><code>inner ```\nstill code\n</code></pre>\n",
    );
}

#[test]
fn opt_space_indented_opening_fence() {
    // kramdown ignores 1–3 leading spaces (OPT_SPACE) before the opener; the
    // info string is read from the de-indented fence. Common shape: a fence
    // nested under a Liquid block's indentation.
    ok(
        "  ```js\nconst x = 1;\n  ```\n",
        "<pre><code class=\"language-js\">const x = 1;\n</code></pre>\n",
    );
    // 3-space opener + 3-space close, no info.
    ok("   ```\ndeep\n   ```\n", "<pre><code>deep\n</code></pre>\n");
    // Tilde fence, indented.
    ok("  ~~~\ntilde\n  ~~~\n", "<pre><code>tilde\n</code></pre>\n");
}

#[test]
fn opt_space_fence_content_is_verbatim() {
    // The body is NOT de-indented — content lines are kept exactly, even when
    // more or less indented than the opening fence.
    ok(
        "  ```ruby\n    deeper\n  level\n  ```\n",
        "<pre><code class=\"language-ruby\">    deeper\n  level\n</code></pre>\n",
    );
}

#[test]
fn info_string_is_a_single_token() {
    // kramdown's info is one `\S+` token; the language class drops a `?query`
    // and keeps other punctuation verbatim.
    ok(
        "~~~ruby?foo=1\nx\n~~~\n",
        "<pre><code class=\"language-ruby\">x\n</code></pre>\n",
    );
    ok(
        "~~~{:.cls}\nx\n~~~\n",
        "<pre><code class=\"language-{:.cls}\">x\n</code></pre>\n",
    );
    ok("~~~c++ \nx\n~~~\n", "<pre><code class=\"language-c++\">x\n</code></pre>\n");
    // A second token (internal whitespace) means it is NOT a fence opener — the
    // run isn't a closed fence, so kramdown renders the lines as one paragraph
    // (the `~~~` runs stay literal: a closing `~~~` on its own line is preceded
    // by a newline, so it can't close a strikethrough).
    ok(
        "~~~ruby extra\nx\n~~~\n",
        "<p>~~~ruby extra\nx\n~~~</p>\n",
    );
    ok(
        "~~~{% raw %}\ncode\n~~~\n",
        "<p>~~~{% raw %}\ncode\n~~~</p>\n",
    );
}

#[test]
fn fence_with_internal_blank_inside_a_list_item() {
    // A fenced code block carried as a trailing block of a list item, whose
    // BODY contains a blank line. The recursive item parse records that blank
    // as a `""` placeholder (not a sub-slice of `src`); the fence body must
    // still reconstruct verbatim — the contiguity check treats the `""` as
    // non-abutting and joins an owned body rather than underflowing the
    // pointer arithmetic. Byte-identical to kramdown (GFM profile).
    ok(
        "1. text\n\n   ```sh\n   a\n\n   b\n   ```\n",
        "<ol>\n  <li>\n    <p>text</p>\n\n    <pre><code class=\"language-sh\">a\n\nb\n</code></pre>\n  </li>\n</ol>\n",
    );
}

#[test]
fn opt_space_open_and_close_indents_may_differ() {
    // The opener's and closer's 0–3 indents are independent.
    ok(
        "   ```\nmismatch\n```\n",
        "<pre><code>mismatch\n</code></pre>\n",
    );
    ok(
        "```\nplain\n   ```\n",
        "<pre><code>plain\n</code></pre>\n",
    );
}
