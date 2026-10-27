# FMX patch to epaint 0.37.0

This is epaint 0.37.0 from crates.io with right-to-left text support added,
used through `[patch.crates-io]` in FMX's `Cargo.toml`. Upstream epaint has
no bidirectional text support yet (see `src/text/unicode.rs`: "TODO: heed
bidi characters"), so Hebrew renders with its letters and words reversed.

## What changed

- `src/text/bidi.rs` (new): the Unicode Bidirectional Algorithm steps epaint
  lacks, using the `unicode-bidi` and `unicode-bidi-mirroring` crates:
  bracket mirroring in right-to-left runs (rule L4) and per-row visual
  reordering (rules L1 and L2).
- `src/text/text_layout.rs`, `layout()`: shapes with mirrored text, and
  reorders rows after alignment. Marked `FMX patch`.
- `src/text/text_layout.rs`, `shape_text()`: forces left-to-right shaping, so
  glyphs stay in logical order for line breaking. Marked `FMX patch`.
- `src/text/mod.rs`: declares `mod bidi`.
- `Cargo.toml`: adds the two crates above.

Text with no right-to-left characters takes none of the new paths. Hebrew
needs no contextual shaping, so left-to-right shaping is exact for it; Arabic
would also need right-to-left shaping of its joining forms.

## Upgrading egui

1. Replace this folder with the new epaint release from crates.io.
2. Re-apply the changes above (search the old copy for `FMX patch`).
3. Run FMX's `cargo test`, and epaint's own tests from this folder:
   `cargo test --lib --features default_fonts` (delete the `Cargo.lock` it
   writes here afterwards). Then check the Hebrew UI with `FMX_LANG=he`.

If a future epaint supports bidi text itself, delete this folder and the
`[patch.crates-io]` entry instead.
