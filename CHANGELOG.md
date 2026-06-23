# Changelog

All notable changes to `rostdown` are documented here. The project targets
**byte-identical** output to kramdown 2.5.2 (GFM input, Jekyll defaults) for an
explicitly-bounded subset; out-of-subset constructs return `Error::Declined`.

## [0.1.1] - 2026-06-23

A performance release. **Output is byte-identical to 0.1.0** — the full
acceptance gate (golden corpus + the gem's 331-case Jekyll/Bridgetown
differential) is green, `WRONG = 0`, at every commit. No public API changed.

### Performance

Default build throughput on the 37.5 KB synthetic prose corpus rose
**~282 → ~424 MB/s (+50 %)** (Apple M2 Max); the opt-in `arena` build reaches
~459. rostdown now beats pulldown-cmark in both builds, and on real
Jekyll/Bridgetown pages (e.g. `themes.md` +32 %, the link-heavy `history.md`
+12 % even as the slowest real outlier). Highlights:

- **Local bump arena** (`src/bump.rs`): the AST's owned strings (typography
  rewrites, de-prefixed blockquote/list bodies, re-serialized HTML) live in a
  bump freed wholesale; allocs/render 1160 → 780. First chunk pre-sized from
  `src.len()`.
- **Blockquote/list deep-own parses straight into the arena** instead of
  building a scratch AST and copying every node in (four functions deleted).
- **`SpanKind` holds `&'a str` instead of `Cow`** → `SpanNode` is a POD with no
  per-node `Drop`.
- **Letter-first fast-paths** through the block-opener and
  paragraph-continuation loops skip the trim-heavy `is_hr` / `decline_block_scan`
  probes for ordinary prose lines (amplified by `parse_blocks`'s recursion
  through list items and blockquote bodies).
- **SWAR / byte-level scans**: inline hard-break (`memchr`), `escape_attr`
  (`memchr4`), `leading_spaces` (replacing 17 `trim_start_matches(' ')` sites),
  `is_hr` (trim start only), `gfm_slug` / `gfm_slug_ok` (ASCII fast path), and a
  `]:` fast-bail that skips the link-definition pre-pass entirely.

### Changed — `unsafe` posture (not an API or output break)

The **default build now contains `unsafe`**, where 0.1.0's default was
`unsafe`-free:

- The local bump arena is now **always on** (one contained, Miri-checked
  module — Stacked + Tree Borrows, strict provenance).
- On **aarch64**, the NEON inline-trigger scan is now the **default** (NEON is a
  baseline instruction set there, so the `unsafe` is a formality); other
  targets keep the scalar table.

Both are byte-identical to the scalar/safe paths (pinned by tests) and Miri-clean.
Consumers requiring an `unsafe`-free build should pin `0.1.0`.

- The **`simd` feature is now inert** (a no-op kept so existing
  `features = ["simd"]` deps still build); the NEON scan it used to gate is now
  default on aarch64.

## [0.1.0] - baseline

First correctness-complete engine: 100 % byte-identical acceptance on the full
Jekyll + Bridgetown corpus, with a zero-dependency, `unsafe`-free default build
(`arena` + `simd` opt-in). ~282 MB/s default on the synthetic corpus.
