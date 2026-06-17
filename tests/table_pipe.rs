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
    declined("> quote | pipe\n"); // pipe inside a blockquote → <blockquote><table>
}

#[test]
fn pipe_line_interrupted_by_block_is_a_paragraph() {
    // kramdown forms a table only when the rows end at a blank/EOF. A pipe
    // line cut short by a block-starter is a paragraph (with literal pipes),
    // even a separator row can't rescue it.
    ok(
        "x | y | z\n- item\n",
        "<p>x | y | z</p>\n<ul>\n  <li>item</li>\n</ul>\n",
    );
    ok(
        "x | y | z\n# heading\n",
        "<p>x | y | z</p>\n<h1 id=\"heading\">heading</h1>\n",
    );
    ok(
        "a | b\n---|---\nc | d\n- item\n",
        "<p>a | b\n—|—\nc | d</p>\n<ul>\n  <li>item</li>\n</ul>\n",
    );
}

#[test]
fn multiline_pipe_list_item() {
    // An item whose continuation lines are ALSO pipe-rows builds a multi-row
    // table inside the `<li>` (the recursive item parse runs the table builder
    // over all the item's lines).
    ok(
        "* a | b\n  cont | row\n",
        "<ul>\n  <li>\n    <table>\n      <tbody>\n        <tr>\n          <td>a</td>\n          <td>b</td>\n        </tr>\n        <tr>\n          <td>cont</td>\n          <td>row</td>\n        </tr>\n      </tbody>\n    </table>\n  </li>\n</ul>\n",
    );
    // A lazy continuation with NO pipe makes the item's block a plain
    // paragraph (not every line is a row), pipes kept literal.
    ok(
        "* a | b\n{% endfor %}\n",
        "<ul>\n  <li>a | b\n{% endfor %}</li>\n</ul>\n",
    );
}

#[test]
fn pipe_in_list_item_builds_per_item_table() {
    // kramdown's GFM table trigger fires inside a `<li>` too: a single-line
    // item with a `|` becomes a one-row `<table>`, rendered in block form.
    ok(
        "* a | b\n* c | d\n",
        "<ul>\n  <li>\n    <table>\n      <tbody>\n        <tr>\n          <td>a</td>\n          <td>b</td>\n        </tr>\n      </tbody>\n    </table>\n  </li>\n  <li>\n    <table>\n      <tbody>\n        <tr>\n          <td>c</td>\n          <td>d</td>\n        </tr>\n      </tbody>\n    </table>\n  </li>\n</ul>\n",
    );
    // Mixed with a normal (pipe-less) item.
    ok(
        "* plain\n* x | y\n",
        "<ul>\n  <li>plain</li>\n  <li>\n    <table>\n      <tbody>\n        <tr>\n          <td>x</td>\n          <td>y</td>\n        </tr>\n      </tbody>\n    </table>\n  </li>\n</ul>\n",
    );
}

#[test]
fn pipe_inside_inline_html_is_not_a_table() {
    // A `|` inside an inline HTML tag's attribute is not a table separator —
    // the line is an ordinary paragraph (the element is whole).
    ok(
        "Refer <a href=\"/x?a=1|2\">opts</a> here.\n",
        "<p>Refer <a href=\"/x?a=1|2\">opts</a> here.</p>\n",
    );
    ok(
        "x <a href=\"a|b\">link</a> y\n",
        "<p>x <a href=\"a|b\">link</a> y</p>\n",
    );
}
