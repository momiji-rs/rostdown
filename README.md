# rostdown

[![CI](https://github.com/momiji-rs/rostdown/actions/workflows/ci.yml/badge.svg)](https://github.com/momiji-rs/rostdown/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.95%2B-blue.svg)](Cargo.toml)
[![dependencies](https://img.shields.io/badge/dependencies-0-brightgreen.svg)](Cargo.toml)
[![unsafe](https://img.shields.io/badge/unsafe-opt--in-brightgreen.svg)](#features-all-off-by-default)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A [kramdown](https://kramdown.gettalong.org/)-compatible Markdown renderer
in Rust. *Kram is German for stuff; Rost is German for rust.*

rostdown targets **byte-identical HTML** with Ruby kramdown (GFM input
flavor, Jekyll's default options) for a growing subset of the language.
Anything outside the implemented subset is a clean **decline**
([`Error::Declined`]) rather than a guess — embedders fall back to Ruby
kramdown for that document, so output is never silently wrong.

```rust
use rostdown::{to_html, Options, NoHighlight};

let html = to_html("## Hi\n\nSome *text*.\n", &Options::jekyll(), &mut NoHighlight).unwrap();
assert_eq!(html, "<h2 id=\"hi\">Hi</h2>\n\n<p>Some <em>text</em>.</p>\n");
```

## Supported subset

- Block: ATX headings (with kramdown's `auto_ids` slugs), paragraphs,
  unordered/ordered lists, blockquotes, GFM fenced code blocks,
  horizontal rules.
- Span: emphasis/strong, code spans, inline links, backslash escapes.
- Typography: kramdown's smart quotes and typographic symbols
  (`--`/`---`/`...`), `entity_output: as_char`.
- Code blocks route through a [`CodeHighlighter`] hook (plug in a
  rouge-compatible highlighter such as
  [carmine](https://crates.io/crates/carmine), or none for plain
  `<pre><code>`).

## Performance

rostdown is a **zero-copy, flat-node, byte-scanning** engine: the
`Block`/`Span` tree borrows text straight from the source (`Cow<&str>`)
instead of copying it, the nodes live in flat index arenas (no per-node
`Vec` allocation), and the hot scans use word-at-a-time (SWAR) byte
search.

Rendering a 37.5 KB prose-heavy corpus, no syntax highlighting, **both
engines doing smart typography** (Apple M2 Max, median of 7 × 1000
iterations):

| engine                        |   MB/s | ns/render | vs pulldown |
| ----------------------------- | -----: | --------: | ----------: |
| **rostdown** — `arena`+`simd`  | **578** |      65 k |     **+65 %** |
| **rostdown** — default build   | **435** |      86 k |     **+24 %** |
| pulldown-cmark 0.12            |    351 |     107 k |           — |
| comrak 0.29                    |     88 |     425 k |       −75 % |

pulldown is run with `ENABLE_SMART_PUNCTUATION` so its typographic work
(`---`→—, `"x"`→“x”, `...`→…) matches rostdown's — a fair fight, verified
to emit the same curly quotes / dashes / ellipses (pulldown costs ~10 %
throughput for it; rostdown does it inside its single scan). The
**default build — no `unsafe`, no dependencies** (exactly what ships in
the [kramdown-rostdown](https://github.com/linyiru/rubyrs) gem) still
renders faster, *and* does one thing pulldown has no feature for:
kramdown-style **heading `id` auto-slugs** (`# Hello World` →
`<h1 id="hello-world">`; pulldown emits an `id` only from explicit
`{#id}` syntax, never from the heading text). It also decline-checks and
targets byte-identical kramdown HTML — ~5 % more bytes (46 KB vs 44 KB),
mostly those ids. The opt-in `arena`+`simd` features add ~33 % on top
(≈ +65 % over pulldown), but they are headroom, not required to win.

Reproduce with the [rubyrs](https://github.com/linyiru/rubyrs) project's
`poc/markdown-bench` harness (where rostdown originated), which also
benchmarks goldmark, blackfriday, marked, markdown-it and Ruby kramdown.

## Features (all off by default)

- **`arena`** — exposes [`ScopedAlloc`], a scoped bump allocator an
  embedder installs as its `#[global_allocator]`; `to_html` then
  pointer-bumps its AST and frees it wholesale, for extra throughput.
  This is the crate's only `unsafe` module: off by default, covered by
  Miri (Stacked + Tree Borrows) and AddressSanitizer over the live path,
  and `to_html` pauses the arena around highlighter callbacks so their
  allocations survive the reset.
- **`simd`** — an aarch64 NEON byteset for the inline scanner's
  ordinary-text skip (scalar lookup table otherwise). Adds `unsafe`.
- **`profiling`** — exposes `profile_phases()` for parse-vs-convert
  timing. Diagnostic only.

## Testing

The golden corpus under `tests/corpus` is vendored from kramdown's own
test suite (MIT, © Thomas Leitner and contributors); the test runner
reports an implemented-directory dashboard so coverage growth is
measurable. The contract is *right-or-declined* — a render that mismatches
the expected HTML is a hard failure, never accepted. With `--features
arena` the corpus runs through the live bump-allocator path (the byte-
identity gate for the arena that Miri can't reach).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. The vendored kramdown test
corpus under `tests/corpus` is MIT, © Thomas Leitner and contributors.
