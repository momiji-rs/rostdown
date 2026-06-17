//! Indented code blocks (4-space indent) under the gem profile
//! (`Options::jekyll()`): a run of ≥4-space-indented lines at a block
//! boundary becomes `<pre><code>` with exactly four spaces stripped per
//! line; interior blank lines are kept when more indented code follows;
//! content is HTML-escaped. Expected HTML mirrors kramdown 2.5.2.

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
fn basic_run() {
    ok(
        "para\n\n    code line\n    code two\n\nafter\n",
        "<p>para</p>\n\n<pre><code>code line\ncode two\n</code></pre>\n\n<p>after</p>\n",
    );
}

#[test]
fn single_line() {
    ok("    only code\n", "<pre><code>only code\n</code></pre>\n");
}

#[test]
fn interior_blank_kept() {
    ok(
        "    a\n\n    b\n",
        "<pre><code>a\n\nb\n</code></pre>\n",
    );
}

#[test]
fn escaping_and_indent() {
    // `<`/`>`/`&` escaped; exactly four spaces stripped (extra kept).
    ok(
        "    has  <>&  chars\n",
        "<pre><code>has  &lt;&gt;&amp;  chars\n</code></pre>\n",
    );
    ok("        deep eight\n", "<pre><code>    deep eight\n</code></pre>\n");
}

#[test]
fn after_heading() {
    ok("# H\n    code\n", "<h1 id=\"h\">H</h1>\n<pre><code>code\n</code></pre>\n");
}
