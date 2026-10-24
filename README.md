# Muxr

A lightweight, fast all-in-one converter written in Rust. It is a single ~14 MB native binary with an egui GUI (OpenGL/glow, no web engine),
available in English, Spanish, French and German.

## Tools

| Category | Tools |
|---|---|
| Video | MP4 (H.264), MP4 (H.265), MKV, WebM, MOV, AVI, FLV, WMV, MPG, VOB, TS, OGV, 3GP, SWF, GIF, animated WebP |
| Mobile devices | iPhone, iPad, Android phone, Android tablet, PSP, PS Vita, Xbox, Smart TV, YouTube, WhatsApp |
| Audio | MP3, AAC, M4A, M4R ringtone, FLAC, WAV, OGG, Opus, WMA, AIFF, AC3, MP2, WavPack, MKA, MMF, AMR, APE (also extracts audio from video) |
| Image | PNG, JPG, WebP, BMP, TIFF, GIF, ICO, TGA, QOI (native), AVIF, JPEG XL, PCX (through ffmpeg); join images; resize, rotate, grayscale and watermark; HEIC/AVIF inputs |
| Document | Images → PDF, PDF → images / TXT / HTML / Word / Excel, Office → PDF, merge / split / compress PDF, eBook → EPUB / MOBI / AZW3 / FB2 / PDF / TXT |
| ROM / DVD / CD / ISO | DVD → video, Blu-ray → video, music CD → audio tracks, DVD/CD → ISO, folder → ISO, ISO ↔ CSO (native) |
| Utilities | Join video, join audio, mux, mix audio, combine two videos (side by side / stacked / picture-in-picture), trim, split, crop, compress to size, remove audio, remove logo, add subtitle track, extract subtitles, extract frames, rotate/flip, speed, volume, loudness normalization, repair, video downloader, screen recorder |

**Video conversions** also have:

- **Output settings:** codec (H.264 / H.265 / AV1 / VP9), NVENC / Quick Sync / VA-API hardware
  encoding, bitrate, frame rate, audio bitrate, sample rate, channels and volume.
- **Edit panel:** clip, crop, rotate, burned-in subtitles and an image watermark.
- **Clip & crop window:** scrub through the video, set start/end points, and drag the crop area
  directly on the picture (eye button next to a file).
- **Hardware decoding** in addition to hardware encoding.

Audio conversions get the same output settings and clipping.

**Interface:** colorful category navigation. Pick a category in the sidebar, then a tool
from a grid of colored cards (or search across all tools). Muxr has a light and a dark theme that follow the system setting (or can be chosen in
Settings), with smooth hover and press animations, gliding progress bars that show the time left,
status chips, and toast notifications. "Reduce motion" in Settings turns the animations off.

**Queue:** jobs run in parallel with progress, cancel and retry. You can save and load task lists,
and choose an action for when everything is done (open the output folder or shut down after a
60-second countdown). Drag & drop and recursive folder adding work everywhere. Output names never
overwrite existing files, and failed jobs leave no partial files behind.

## Requirements

**Nothing has to be installed.**

- **ffmpeg:** media tools use ffmpeg, which Muxr finds in this order: the path set in Settings, next
  to the `muxr` executable, on `PATH`, then Muxr's own copy. If none exists, Muxr offers to
  download a static ffmpeg 6.1.1 build (about 20–30 MB) into its data folder
  (`~/.local/share/muxr` on Linux). The download is checked against its published SHA-256 before
  use, and nothing is installed system-wide.
- **yt-dlp (video downloader):** the first time you open the video downloader, Muxr offers to
  download the official standalone yt-dlp (about 40 MB on Linux) from yt-dlp's latest GitHub
  release into the same data folder. It is checked against the SHA-256 list published with that
  release. Video sites change often, so Settings → Programs has **Update yt-dlp**, and a failed
  download suggests it. Muxr's own copy is preferred over a system `yt-dlp`, which is often too
  old for YouTube.
- **Documents:** Office → PDF is built in. It reads DOCX, ODT, PPTX, ODP, XLSX, XLS, ODS, CSV,
  Markdown, HTML, RTF and text, and writes PDFs with an embedded Unicode font. PDF → Word writes
  DOCX natively. Only old binary `.doc`/`.ppt` files need LibreOffice, and only if you convert them.
- **Images, Images → PDF, PDF → Excel, ISO ↔ CSO, DVD/CD → ISO** are built in.

Optional helpers turn on extra tools when present (found next to the app or on `PATH`):

| Helper | Used for | Debian/Ubuntu package |
|---|---|---|
| Poppler | PDF → images/TXT/HTML/Word/Excel, merge and split PDF | `poppler-utils` |
| Ghostscript | compress PDF | `ghostscript` |
| Calibre | eBook conversions | `calibre` |
| xorriso | folder → ISO | `xorriso` |
| Monkey's Audio | APE output | `monkeys-audio` |
| LibreOffice | legacy `.doc`/`.ppt` → PDF | `libreoffice` |

Formats your ffmpeg build cannot encode, such as AMR without `libopencore_amrnb`, are greyed out.

On Wayland the screen recorder can only capture X11 (XWayland) windows, because ffmpeg has no
Wayland capture. Log in to an X11 session to record the whole desktop.

You can open files straight from a file manager or the command line: `muxr video.mp4 photo.png`.
A file-name tag (Settings) is appended to every output name, and a desktop notification appears
when the queue finishes.

## Build and run

```bash
cargo run --release
```

## Tests

```bash
cargo test
```

End-to-end tests run every tool against the real helper programs when `MUXR_FFMPEG` is set:

```bash
MUXR_FFMPEG=/usr/bin/ffmpeg cargo test e2e -- --nocapture
```

## Configuration

- `MUXR_LANG=es|fr|de|en` overrides the UI language; it can also be changed in **Settings**.
  To add a language, add `locales/xx.json` and register it in `src/i18n.rs`.
- `MUXR_LOG=debug` prints every command line Muxr runs.

## Not covered

- Copy-protected discs: ffmpeg's DVD and Blu-ray readers do not decrypt CSS or AACS.
- A built-in media player. Muxr opens finished files in your default player instead.
