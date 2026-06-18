//! kramdown's `{:toc}` table of contents. A list directly followed by a
//! `{:toc}` block IAL is replaced by a generated `<ul id="markdown-toc">` of
//! links to every heading, nested by heading level. Each entry is
//! `<a href="#id" id="markdown-toc-{id}">heading text</a>`, the leading
//! paragraph transparent (inline). Expected HTML mirrors kramdown 2.5.2 under
//! the gem profile (`Options::jekyll()`, NoHighlight).

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
fn flat_toc_all_same_level() {
    // Same-level headings ⇒ a flat `<ul id="markdown-toc">`; each `<li>` holds
    // the link inline (transparent paragraph).
    ok(
        "* TOC\n{:toc}\n\n## Alpha\n\n## Beta\n",
        "<ul id=\"markdown-toc\">\n  \
           <li><a href=\"#alpha\" id=\"markdown-toc-alpha\">Alpha</a></li>\n  \
           <li><a href=\"#beta\" id=\"markdown-toc-beta\">Beta</a></li>\n\
         </ul>\n\n\
         <h2 id=\"alpha\">Alpha</h2>\n\n\
         <h2 id=\"beta\">Beta</h2>\n",
    );
}

#[test]
fn nested_toc_by_level() {
    // Deeper headings nest in a child `<ul>` (no id), laid out exactly as
    // kramdown: the nested list opens on the parent link's line.
    ok(
        "* TOC\n{:toc}\n\n# A\n\n## B\n\n## C\n\n# D\n",
        "<ul id=\"markdown-toc\">\n  \
           <li><a href=\"#a\" id=\"markdown-toc-a\">A</a>    <ul>\n      \
             <li><a href=\"#b\" id=\"markdown-toc-b\">B</a></li>\n      \
             <li><a href=\"#c\" id=\"markdown-toc-c\">C</a></li>\n    \
           </ul>\n  </li>\n  \
           <li><a href=\"#d\" id=\"markdown-toc-d\">D</a></li>\n\
         </ul>\n\n\
         <h1 id=\"a\">A</h1>\n\n\
         <h2 id=\"b\">B</h2>\n\n\
         <h2 id=\"c\">C</h2>\n\n\
         <h1 id=\"d\">D</h1>\n",
    );
}

#[test]
fn toc_ordered_marker_keeps_ol() {
    // The generated TOC keeps the `{:toc}` list's own type: an ordered marker
    // yields `<ol id="markdown-toc">`. Content text ("anything") is ignored.
    ok(
        "1. anything\n{:toc}\n\n## Only\n",
        "<ol id=\"markdown-toc\">\n  \
           <li><a href=\"#only\" id=\"markdown-toc-only\">Only</a></li>\n\
         </ol>\n\n\
         <h2 id=\"only\">Only</h2>\n",
    );
}

#[test]
fn toc_dedups_repeated_heading_ids() {
    // The TOC anchors use the same deduped ids the headings get (`-1`, …).
    ok(
        "* TOC\n{:toc}\n\n## Dup\n\n## Dup\n",
        "<ul id=\"markdown-toc\">\n  \
           <li><a href=\"#dup\" id=\"markdown-toc-dup\">Dup</a></li>\n  \
           <li><a href=\"#dup-1\" id=\"markdown-toc-dup-1\">Dup</a></li>\n\
         </ul>\n\n\
         <h2 id=\"dup\">Dup</h2>\n\n\
         <h2 id=\"dup-1\">Dup</h2>\n",
    );
}

#[test]
fn toc_heading_with_link_declines() {
    // kramdown unwraps a link inside a heading for its TOC entry (`See docs`).
    // Reproducing that is out of subset, so we decline rather than emit a
    // nested `<a>` — byte-identical-or-declined.
    declined("* TOC\n{:toc}\n\n## See [docs](/x)\n");
}

#[test]
fn bare_toc_without_list_still_declines() {
    // `{:toc}` not attached to a preceding list is a lonely IAL we don't model.
    declined("text\n\n{:toc}\n\n## H\n");
}
