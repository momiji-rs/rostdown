//! HTML entity resolution — kramdown's `:as_char` output. A named entity in
//! kramdown's table or a valid numeric reference decodes to its character;
//! the three HTML-significant code points (`&`, `<`, `>`) stay escaped, and
//! their NAMED form (`&amp;`/`&lt;`/`&gt;`) round-trips through our text
//! escaper while their NUMERIC form (`&#38;`) declines. Unknown / malformed
//! references leave the `&` literal (→ `&amp;…`), exactly as kramdown does.
//! Expected HTML mirrors kramdown 2.5.2 (gem profile, NoHighlight).

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
fn named_to_char() {
    ok("a &copy; b\n", "<p>a © b</p>\n");
    ok("a &mdash; b\n", "<p>a — b</p>\n");
    ok("a &hellip; b\n", "<p>a … b</p>\n");
    ok("a &raquo;&laquo; b\n", "<p>a »« b</p>\n");
    ok("a &trade; b\n", "<p>a ™ b</p>\n");
    // nbsp is U+00A0, a non-breaking space.
    ok("a &nbsp; b\n", "<p>a \u{a0} b</p>\n");
    // Case-sensitive: Greek capital Alpha.
    ok("a &Alpha; b\n", "<p>a \u{391} b</p>\n");
}

#[test]
fn the_escaped_trio_named() {
    // `&`, `<`, `>` stay escaped; the named form round-trips through the
    // text escaper to the same bytes kramdown emits.
    ok("a &amp; b\n", "<p>a &amp; b</p>\n");
    ok("a &lt; b\n", "<p>a &lt; b</p>\n");
    ok("a &gt; b\n", "<p>a &gt; b</p>\n");
}

#[test]
fn numeric_decimal_and_hex() {
    ok("a &#39; b\n", "<p>a ' b</p>\n"); // apostrophe, literal
    ok("a &#34; b\n", "<p>a \" b</p>\n"); // double quote, literal
    ok("a &#37; b\n", "<p>a % b</p>\n");
    ok("a &#x2014; b\n", "<p>a — b</p>\n"); // em dash via hex
    ok("a &#160; b\n", "<p>a \u{a0} b</p>\n");
}

#[test]
fn c1_numeric_remap() {
    // HTML5 remaps `&#128;` → € (U+20AC), `&#153;` → ™, both dec and hex.
    ok("a &#128; b\n", "<p>a € b</p>\n");
    ok("a &#x80; b\n", "<p>a € b</p>\n");
    ok("a &#153; b\n", "<p>a ™ b</p>\n");
}

#[test]
fn unknown_or_malformed_stays_literal() {
    // Not in kramdown's table / not well-formed ⇒ literal `&` (escaped).
    ok("a &notareal; b\n", "<p>a &amp;notareal; b</p>\n");
    ok("a &foo; b\n", "<p>a &amp;foo; b</p>\n");
    ok("a &amp b\n", "<p>a &amp;amp b</p>\n"); // no semicolon
    ok("a &#xZZ; b\n", "<p>a &amp;#xZZ; b</p>\n"); // bad hex
    ok("a R&D team\n", "<p>a R&amp;D team</p>\n"); // bare ampersand
}

#[test]
fn decoded_char_is_not_remarked_up() {
    // `&#42;` decodes to a literal `*`, never emphasis.
    ok("a &#42;b&#42; c\n", "<p>a *b* c</p>\n");
}

#[test]
fn not_decoded_inside_code() {
    // Code spans / fences keep entities literal (then escape the `&`).
    ok("a `&copy;` b\n", "<p>a <code>&amp;copy;</code> b</p>\n");
}

#[test]
fn numeric_special_and_invalid_decline() {
    // A NUMERIC reference to `&`/`<`/`>` needs the numeric source form on
    // output, which escaping a `&` can't produce — decline.
    declined("a &#38; b\n");
    declined("a &#60; b\n");
    declined("a &#x3e; b\n");
    // Out-of-range code point (kramdown emits U+FFFD replacements).
    declined("a &#999999999; b\n");
}
