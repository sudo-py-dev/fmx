# Muxr

A lightweight, fast media converter written in Rust.
It is a single ~13 MB native binary with an egui GUI (OpenGL/glow, no web engine).

## Tools

| Category  | Tools |
|-----------|-------|
| Video     | MP4 (H.264), MP4 (H.265), MKV, WebM (VP9), MOV, AVI, FLV, WMV, MPG, TS, OGV, 3GP, GIF |
| Audio     | MP3, AAC, M4A, FLAC, WAV, OGG, Opus, WMA, AIFF, AC3 (also extracts audio from video) |
| Image     | PNG, JPG, WebP, BMP, TIFF, GIF, ICO, TGA, QOI: native, no ffmpeg needed |
| Document  | Images → PDF (native) |
| Utilities | Join videos, join audio, mux video+audio, trim/cut (lossless fast mode), compress to target size, remove audio, extract frames, rotate/flip, change speed, normalize loudness |

Other features: batch queue, parallel jobs, per-job progress and cancel, drag & drop, collision-free
output names, partial outputs deleted on failure, and settings that persist between runs.

## Requirements

Media tools use an external **ffmpeg** binary. Muxr looks for it in this order:

1. the path set in **Settings**
2. next to the `muxr` executable
3. `PATH`

The image and PDF tools work without ffmpeg.

## Build and run

```bash
cargo run --release
```

## Tests

```bash
cargo test
```

End-to-end tests run every media tool against a real ffmpeg when you set `MUXR_FFMPEG`:

```bash
MUXR_FFMPEG=/usr/bin/ffmpeg cargo test e2e -- --nocapture
```

## Configuration

- `MUXR_LANG=xx` overrides the UI language (add `locales/xx.json` and register it in `src/i18n.rs`).
- `MUXR_LOG=debug` prints the ffmpeg command lines.

## Not included

DVD/CD ripping, PDF → image/text, and office-document conversion are not
implemented. They need heavy native libraries (libdvdread, pdfium, LibreOffice), which would go
against the "lite" goal.
