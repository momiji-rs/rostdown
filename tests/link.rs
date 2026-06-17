//! Inline links with nested content in the link text. kramdown finds the
//! matching `]` tracking bracket depth (so balanced nested brackets and a
//! linked image `[![…](…)](…)` stay inside the text) and parses the text as
//! inline content — but forbids NESTED LINKS (`[x](y)` inside link text is
//! literal). Expected HTML mirrors kramdown 2.5.2 (gem profile).

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
fn linked_image() {
    ok(
        "[![alt](/img.png)](/link)\n",
        "<p><a href=\"/link\"><img src=\"/img.png\" alt=\"alt\" /></a></p>\n",
    );
}

#[test]
fn balanced_nested_brackets_stay_literal() {
    ok(
        "see [text [inner] more](/url) ok\n",
        "<p>see <a href=\"/url\">text [inner] more</a> ok</p>\n",
    );
}

#[test]
fn bracket_inside_code_span_in_link_text() {
    ok(
        "[`cmd [a|b]`](/d)\n",
        "<p><a href=\"/d\"><code>cmd [a|b]</code></a></p>\n",
    );
}

#[test]
fn escaped_bracket_in_link_text() {
    ok("[a\\]b](/url)\n", "<p><a href=\"/url\">a]b</a></p>\n");
}

#[test]
fn no_nested_links() {
    // `[x](y)` inside a link's text is NOT a link — it stays literal.
    ok(
        "[URL with [no](link.html) inside](/u)\n",
        "<p><a href=\"/u\">URL with [no](link.html) inside</a></p>\n",
    );
}

#[test]
fn balanced_parens_in_destination() {
    // kramdown depth-matches the destination's parens: a `(` nests and the
    // link closes at the `)` seen at depth 0.
    ok(
        "[Fork](https://x/Fork_(software_development))\n",
        "<p><a href=\"https://x/Fork_(software_development)\">Fork</a></p>\n",
    );
    // A doubled opening paren keeps the inner one in the href.
    ok(
        "[t]((https://x/y))\n",
        "<p><a href=\"(https://x/y)\">t</a></p>\n",
    );
}

#[test]
fn quirky_destinations_decline() {
    // An angle-bracket destination needs kramdown's separate `<…>` scan —
    // declined (safe), not truncated.
    declined("[y](<a b>)\n");
}

#[test]
fn smart_quote_after_code_span_declines() {
    // A `'`/`"` directly after a code span sits on a boundary kramdown
    // classifies with its SQ_RULES (`` `x`'d `` opens, `` `x`'s `` closes) —
    // out of subset, declined rather than mis-quoted. (A code span NOT
    // followed by a quote, and a normal smart quote, still render.)
    declined("a `code`'d here\n");
    ok("a `code` then text\n", "<p>a <code>code</code> then text</p>\n");
}

#[test]
fn depth_matched_dest_with_title_and_edges() {
    // A title after a paren-containing dest, and a `)` inside the title quote.
    ok(
        "[t](a(b) \"the title\")\n",
        "<p><a href=\"a(b)\" title=\"the title\">t</a></p>\n",
    );
    ok(
        "[t](a \"b)c\")\n",
        "<p><a href=\"a\" title=\"b)c\">t</a></p>\n",
    );
    // The first depth-0 `)` closes; trailing parens stay literal.
    ok(
        "[t](url) and (more).\n",
        "<p><a href=\"url\">t</a> and (more).</p>\n",
    );
    // An empty title `''`/`""` is not a title (kramdown's `.+?`) — literal.
    declined("[t](u '')\n");
}
