//! The pipe / table boundary. kramdown turns a block into a `<table>` ONLY
//! when EVERY line of the block is a row (carries a top-level pipe); a block
//! with any pipe-less line is a plain paragraph with literal pipes. We
//! reproduce the paragraph case and the clean / body-only table; the quirky
//! ones kramdown still tables (ragged rows, pipes inside a list item or
//! blockquote) decline. Expected HTML mirrors kramdown 2.5.2 (gem profile).

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
fn pipe_amid_prose_is_a_paragraph() {
    // A pipe-less line in the block ⇒ paragraph, pipes literal — NOT a table
    // and NOT a decline.
    ok(
        "prose line\na | b\nmore prose\n",
        "<p>prose line\na | b\nmore prose</p>\n",
    );
}

#[test]
fn liquid_filter_pipe_in_prose() {
    // The real-world blocker: an unexpanded Liquid filter pipe in prose. The
    // broken link stays literal (with smart quotes) inside the paragraph.
    ok(
        "See [G]({{ \"/x\" | url }}) ok.\nnext line.\n",
        "<p>See [G]({{ \u{201c}/x\u{201d} | url }}) ok.\nnext line.</p>\n",
    );
}

#[test]
fn every_line_a_row_is_a_body_table() {
    // A single pipe line (and multi-row all-pipe blocks) ARE tables.
    ok(
        "word | other\n",
        "<table>\n  <tbody>\n    <tr>\n      <td>word</td>\n      <td>other</td>\n    </tr>\n  </tbody>\n</table>\n",
    );
}

#[test]
fn clean_header_table_still_renders() {
    ok(
        "a | b\n---|---\nc | d\n",
        "<table>\n  <thead>\n    <tr>\n      <th>a</th>\n      <th>b</th>\n    </tr>\n  </thead>\n  <tbody>\n    <tr>\n      <td>c</td>\n      <td>d</td>\n    </tr>\n  </tbody>\n</table>\n",
    );
}

#[test]
fn quirky_tables_decline() {
    // kramdown tables these, but outside our clean shape — decline (safe):
    declined("a | b | c\nd | e\n"); // ragged (rows differ in cell count)
    declined("* a | b\n* c | d\n"); // pipe inside a list item → <li><table>
    declined("> quote | pipe\n"); // pipe inside a blockquote → <blockquote><table>
}
