//! kramdown smart typography: smart quotes (`lsquo,rsquo,ldquo,rdquo`)
//! plus the typographic symbols `--`/`---`/`...`, rendered with
//! `entity_output: as_char` — i.e. the Unicode characters themselves.
//! Quote contexts we cannot classify with certainty decline instead of
//! guessing.

use crate::Error;

pub(crate) const NDASH: char = '\u{2013}'; // –
pub(crate) const MDASH: char = '\u{2014}'; // —
pub(crate) const HELLIP: char = '\u{2026}'; // …
const LSQUO: char = '\u{2018}'; // ‘
const RSQUO: char = '\u{2019}'; // ’
const LDQUO: char = '\u{201C}'; // “
const RDQUO: char = '\u{201D}'; // ”

/// Context where a quote OPENS: start of text, or after whitespace, an
/// ASCII word char (Ruby `\w`, ASCII mode): `[A-Za-z0-9_]`.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// kramdown's `SQ_PUNCT` punctuation class (ASCII punctuation minus `&`).
fn is_sq_punct(c: char) -> bool {
    c.is_ascii_punctuation() && c != '&'
}

/// kramdown's `SQ_CLOSE`: a preceding char that closes a quote — anything
/// except space, tab, CR, LF, `[`, `{`, `(`, `-`, or a backslash.
fn is_close(c: char) -> bool {
    !matches!(c, ' ' | '\t' | '\r' | '\n' | '[' | '{' | '(' | '-' | '\\')
}

/// Smart-quote classification ported from kramdown's `SQ_RULES`
/// (SmartyPants/RubyPants) — a fixed, ordered rule table over the char
/// before the quote (`prev`) and the text after it (`rest`). kramdown's
/// scanner consumes one non-backslash char before the quote, so the
/// quote-anchored rules (1, 2, 8) only fire when no such char precedes it
/// (`at_start`). The two-character nested-quote forms (`"'`/`'"`) decline.
pub(crate) fn smart_quote(
    prev: Option<char>,
    is_single: bool,
    rest: &str,
) -> Result<char, Error> {
    let (lq, rq) = if is_single {
        (LSQUO, RSQUO)
    } else {
        (LDQUO, RDQUO)
    };
    let at_start = matches!(prev, None | Some('\\'));
    let sp = matches!(prev, Some(c) if c.is_whitespace());
    let mut chars = rest.chars();
    let c0 = chars.next();
    let c1 = chars.next();

    // R1: `'`/`"` directly before emphasis markup (`_`/`*`) → opening.
    if at_start
        && matches!(c0, Some('_' | '*'))
        && {
            let after = if matches!(c1, Some('_' | '*')) {
                chars.next()
            } else {
                c1
            };
            after.is_some_and(|c| !c.is_whitespace())
        }
    {
        return Ok(lq);
    }
    // R2: quote then punctuation (not `..`) at a non-word boundary → close.
    if at_start
        && c0.is_some_and(is_sq_punct)
        && !rest.starts_with("..")
        && !c1.is_some_and(is_word)
    {
        return Ok(rq);
    }
    // R3/R4: `"'`/`'"` before a word — a nested quote pair (two chars).
    // Out of our one-char-at-a-time emit model; decline.
    if (sp || at_start)
        && c1.is_some_and(is_word)
        && ((!is_single && c0 == Some('\'')) || (is_single && c0 == Some('"')))
    {
        return Err(Error::Declined("nested-quote"));
    }
    // R5: `'` before a two-digit decade (`'80s`) → closing apostrophe.
    if is_single
        && (sp || at_start)
        && c0.is_some_and(|c| c.is_ascii_digit())
        && c1.is_some_and(|c| c.is_ascii_digit())
        && chars.next() == Some('s')
    {
        return Ok(RSQUO);
    }
    // R6: whitespace before, word after → opening.
    if sp && c0.is_some_and(is_word) {
        return Ok(lq);
    }
    // R7: a closing char before → closing.
    if prev.is_some_and(is_close) {
        return Ok(rq);
    }
    // R8: quote then whitespace / `s` word-break / end-of-text → closing.
    if at_start
        && match c0 {
            None => true,
            Some(c) if c.is_whitespace() => true,
            Some('s') => !c1.is_some_and(is_word),
            _ => false,
        }
    {
        return Ok(rq);
    }
    // R9/R10: anything else → opening.
    Ok(lq)
}

#[cfg(test)]
mod tests {
    use super::*;

    // All answers verified byte-identical against kramdown 2.5.2
    // (GFM input, auto_ids/hard_wrap off) by the 1166-case differential.
    #[track_caller]
    fn q(prev: Option<char>, single: bool, rest: &str) -> char {
        smart_quote(prev, single, rest).expect("should classify")
    }

    #[test]
    fn smart_quote_rules() {
        // R7: a word/close char before → closing (apostrophes, possessive)
        assert_eq!(q(Some('t'), true, "s"), RSQUO); // it's
        assert_eq!(q(Some('o'), true, ""), RSQUO); // foo'
        assert_eq!(q(Some('`'), true, "s API"), RSQUO); // `code`'s
        assert_eq!(q(Some('#'), false, "x"), RDQUO); // #"x
        assert_eq!(q(Some('§'), false, "Multi"), RDQUO); // §"…
        // R9/R10: opening fallback
        assert_eq!(q(None, true, "foo"), LSQUO); // 'foo (start)
        assert_eq!(q(None, false, "foo"), LDQUO); // "foo (start)
        assert_eq!(q(Some(' '), true, " b"), LSQUO); // space ' space → opening
        // R6: whitespace before, word after → opening
        assert_eq!(q(Some(' '), false, "hi"), LDQUO); // a "hi"
        // R8: start, then whitespace / s-wordbreak / EOL → closing
        assert_eq!(q(None, true, " foo"), RSQUO); // '·foo
        assert_eq!(q(None, true, "s "), RSQUO); // 's (Custer's)
        // R5: decade abbreviation
        assert_eq!(q(None, true, "80s"), RSQUO); // '80s
        assert_eq!(q(Some(' '), true, "90s"), RSQUO); // the '90s
        // R1: before emphasis markup → opening
        assert_eq!(q(None, true, "*em*"), LSQUO);
        // R2: start then punctuation → closing
        assert_eq!(q(None, true, ")."), RSQUO);
    }

    #[test]
    fn nested_quote_pairs_decline() {
        // R3/R4 emit two chars at once — out of our one-char model.
        assert!(smart_quote(None, false, "'inner").is_err()); // "'…
        assert!(smart_quote(None, true, "\"inner").is_err()); // '"…
    }
}
