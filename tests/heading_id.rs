//! GFM heading auto-id slugs for headings carrying symbols/emoji. Under
//! kramdown's rule (`gsub(/[^\p{Word}\- ]/u, '')` + downcase + space→`-`)
//! Symbol-category codepoints are non-Word and dropped just like ASCII
//! punctuation — but the space PRECEDING a trailing emoji is kept and
//! becomes a hyphen, so these slugs end in `-`. Expected ids are literal
//! mirrors of `Kramdown::Document.new(src, input: "GFM", auto_ids: true)`.

use rostdown::{Error, NoHighlight, Options, to_html};

fn render(src: &str) -> Result<String, Error> {
    to_html(src, &Options::jekyll(), &mut NoHighlight)
}

#[track_caller]
fn id_is(src: &str, expected_id: &str) {
    match render(src) {
        Ok(html) => {
            let needle = format!(" id=\"{expected_id}\"");
            assert!(html.contains(&needle), "want {needle:?} in {html:?}");
        }
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
fn trailing_emoji_dropped_space_kept() {
    // 😏 (U+1F60F) is dropped; the space before it survives as a hyphen.
    id_is("### Signals (Of Course 😏)\n", "signals-of-course-");
    id_is(
        "### Customize All the Things, Now in Ruby 😎\n",
        "customize-all-the-things-now-in-ruby-",
    );
}

#[test]
fn arrow_dropped_like_punctuation() {
    // The arrow → (U+2192) is a Symbol — non-Word, dropped — leaving the
    // two flanking spaces as a doubled hyphen.
    id_is("## cache[key] → value\n", "cachekey--value");
    id_is(
        "## Jekyll::Cache.new(name) → new_cache\n",
        "jekyllcachenewname--new_cache",
    );
}

#[test]
fn misc_technical_and_pictograph_blocks() {
    // ⏩ (U+23E9, Misc Technical) and 📦 (U+1F4E6, pictograph) both drop.
    id_is(
        "### Caveats with Fast Refresh in Development ⏩\n",
        "caveats-with-fast-refresh-in-development-",
    );
    id_is("### Switching from Yarn to NPM 📦\n", "switching-from-yarn-to-npm-");
}

#[test]
fn unclassified_letters_still_decline() {
    // Accented Latin / Cyrillic are \p{Word} letters kramdown keeps after
    // a Unicode downcase we don't approximate — still declined, not
    // mis-slugged.
    declined("## Café\n");
    declined("## Привет\n");
    // A variation selector (U+FE0F) riding an emoji is a \p{Word} mark
    // kramdown keeps; we decline rather than guess.
    declined("## Done \u{2705}\u{FE0F}\n");
}

#[test]
fn trailing_header_id_shorthand() {
    // kramdown's `… {#id}` shorthand sets the id and is stripped from the
    // text; the id is NOT slugged and NOT deduplicated.
    id_is("### With rbenv {#rbenv}\n", "rbenv");
    ok("### With rbenv {#rbenv}\n", "<h3 id=\"rbenv\">With rbenv</h3>\n");
    id_is("###  spaced  {#x}\n", "x");
    // A `{#id}` followed by closing hashes, or glued to a word, or with text
    // after, is literal — the auto-id slugs the whole text.
    id_is("### H {#id} ###\n", "h-id");
    id_is("### a{#id}\n", "aid");
    id_is("### Mid {#id} text here\n", "mid-id-text-here");
    // Braces that aren't the id shorthand stay literal text.
    id_is("### Set {x} to value\n", "set-x-to-value");
    // A trailing `{:.cls}` is NOT an id shorthand; the `{:` span declines.
    declined("## Title {:.note}\n");
}

#[track_caller]
fn ok(src: &str, expected: &str) {
    match render(src) {
        Ok(html) => assert_eq!(html, expected, "input: {src:?}"),
        Err(e) => panic!("declined {src:?}: {e:?}"),
    }
}
