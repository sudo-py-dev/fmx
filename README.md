<div align="center">
  <img src="packaging/icons/fmx-128.png" alt="FMX icon" width="96" />

  # FMX — Free Media Xpress

  **A single native binary that converts video, audio, images, documents and discs.**  
  Built in Rust · egui GUI · No installation needed · ~22 MB

  [![Release](https://img.shields.io/github/v/release/sudo-py-dev/fmx?style=flat-square&color=4a90e2)](https://github.com/sudo-py-dev/fmx/releases/latest)
  [![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue?style=flat-square)](LICENSE-MIT)
  [![Build](https://img.shields.io/github/actions/workflow/status/sudo-py-dev/fmx/release.yml?style=flat-square)](https://github.com/sudo-py-dev/fmx/actions)

  [**Download**](https://github.com/sudo-py-dev/fmx/releases/latest) · [Issues](https://github.com/sudo-py-dev/fmx/issues)

  ---

  ![FMX screenshot](assets/screenshot.png)

</div>

---

## ✨ Features at a glance

- **One window, every format** — video, audio, image, document, disc/ISO and utilities all in one place
- **Zero setup** — ships without ffmpeg; downloads and verifies its own copy on first use
- **Safe auto-updates** — every release file is Ed25519-signed; unsigned or tampered files are refused
- **Parallel queue** — jobs run side by side with live progress, cancel and retry
- **Light & dark theme** — follows system preference; smooth hover/press animations; "Reduce motion" option
- **4 languages** — English, Spanish, French, German (or set `FMX_LANG=xx`)
- **Drag & drop everywhere** — files, folders and links; open files directly from the command line

---

## 🔧 Tools

<details open>
<summary><b>Video</b></summary>

MP4 (H.264 · H.265) · MKV · WebM · MOV · AVI · FLV · WMV · MPG · VOB · TS · OGV · 3GP · SWF · GIF · Animated WebP

**Mobile presets:** iPhone · iPad · Android · PSP · PS Vita · Xbox · Smart TV · YouTube · WhatsApp

**Output settings:** codec (H.264 / H.265 / AV1 / VP9) · NVENC / Quick Sync / VA-API hardware encoding · bitrate · frame rate · audio bitrate · sample rate · channels · volume

**Edit panel:** clip · crop · rotate · burned-in subtitles · image watermark

**Clip & crop window:** scrub through the video, set in/out points, drag the crop area on the live picture

</details>

<details>
<summary><b>Audio</b></summary>

MP3 · AAC · M4A · M4R ringtone · FLAC · WAV · OGG · Opus · WMA · AIFF · AC3 · MP2 · WavPack · MKA · MMF · AMR · APE

Extracts audio from any video file. Clipping and output settings included.

</details>

<details>
<summary><b>Image</b></summary>

PNG · JPG · WebP · BMP · TIFF · GIF · ICO · TGA · QOI · AVIF · JPEG XL · PCX

Join images · resize · rotate · grayscale · watermark · HEIC/AVIF inputs

</details>

<details>
<summary><b>Document</b></summary>

Images → PDF · PDF → images / TXT / HTML / Word / Excel · Office → PDF · merge / split / compress PDF · eBook → EPUB / MOBI / AZW3 / FB2 / PDF / TXT

</details>

<details>
<summary><b>Disc / ISO</b></summary>

DVD → video · Blu-ray → video · Music CD → audio tracks · DVD/CD → ISO · Folder → ISO · ISO ↔ CSO (native)

</details>

<details>
<summary><b>Utilities</b></summary>

Join video · join audio · mux · mix audio · side-by-side / stacked / picture-in-picture · trim · split · crop · compress to size · remove audio · remove logo · add/extract subtitles · extract frames · rotate/flip · speed · volume · loudness normalization · repair · video downloader · screen recorder

</details>

---

## 📦 Install

Download the right file from the [**latest release**](https://github.com/sudo-py-dev/fmx/releases/latest):

| Platform | File |
|---|---|
| **Windows 10/11** (64-bit) | `fmx-<version>-windows-x64-setup.exe` |
| **macOS 11+** (Intel & Apple silicon) | `fmx-<version>-macos-universal.dmg` |
| **Ubuntu / Debian** (64-bit) | `fmx_<version>_amd64.deb` → `sudo apt install ./fmx_<version>_amd64.deb` |
| **Other Linux** (64-bit) | `fmx-<version>-linux-x86_64.tar.gz` → portable, just run `./fmx` |

> **macOS note:** the build is not yet notarized. First launch: right-click FMX → **Open**.

---

## 🔄 Updates

FMX checks GitHub once a day and shows a banner when a newer release is available.  
Updates are safe by design:

- Only talks to this repository's GitHub releases, over HTTPS
- Never offers a downgrade, draft or pre-release
- Every file is verified against a minisign Ed25519 signature that names the exact file and version
- A file that fails the check is deleted and never run

Turn auto-check off, or hit **Check now**, in **Settings → Updates**.

---

## ⚙️ Requirements

**Nothing needs to be installed upfront.**

| Tool | How FMX gets it |
|---|---|
| **ffmpeg** | Finds it next to the binary, on `PATH`, or in its data dir — otherwise offers to download a verified static build (~25 MB) |
| **yt-dlp** | Downloaded from yt-dlp's latest GitHub release on first use, SHA-256 verified |

Optional helpers that unlock extra tools when found next to the app or on `PATH`:

| Helper | Unlocks | Debian/Ubuntu package |
|---|---|---|
| Poppler | PDF → images/TXT/HTML/Word/Excel, merge/split PDF | `poppler-utils` |
| Ghostscript | Compress PDF | `ghostscript` |
| Calibre | eBook conversions | `calibre` |
| xorriso | Folder → ISO | `xorriso` |
| Monkey's Audio | APE output | `monkeys-audio` |
| LibreOffice | Legacy `.doc`/`.ppt` → PDF | `libreoffice` |

> Formats unsupported by your ffmpeg build (e.g. AMR without `libopencore_amrnb`) are greyed out automatically.

---

## 🛠 Build from source

```bash
# Run in development mode
cargo run --release

# Run the test suite
cargo test

# End-to-end tests (requires ffmpeg on PATH or FMX_FFMPEG set)
FMX_FFMPEG=/usr/bin/ffmpeg cargo test e2e -- --nocapture
```

Rust 1.88+ is required (`rust-version` in `Cargo.toml`).

---

## 🚀 Making a release

1. Bump `version` in `Cargo.toml` (the only place — installers and the updater read it from there).
2. Tag and push:
   ```bash
   git tag v0.0.5 && git push origin master v0.0.5
   ```
3. The [release workflow](.github/workflows/release.yml) builds on Linux, Windows and macOS, signs every artifact with `FMX_SIGNING_KEY`, verifies against `packaging/update-key.pub`, and publishes the release.

---

## 🌐 Configuration

| Variable | Effect |
|---|---|
| `FMX_LANG=es\|fr\|de\|en` | Override UI language (also changeable in Settings) |
| `FMX_LOG=debug` | Print every command FMX runs |
| `FMX_FFMPEG=<path>` | Force a specific ffmpeg binary |

To add a language, add `locales/xx.json` and register it in `src/i18n.rs`.

---

## ⚠️ Known limitations

- **Copy-protected discs:** ffmpeg does not decrypt CSS (DVD) or AACS (Blu-ray).
- **Screen recorder on Wayland:** can only capture XWayland windows; log in to an X11 session to record the full desktop.
- **No built-in player:** finished files open in your default system player.

---

## 📄 License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) — your choice.
