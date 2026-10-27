<div align="center">
  <img src="packaging/icons/fmx-128.png" alt="FMX icon" width="96" />

  # FMX — Free Media Xpress

  **A native app that converts video, audio, images, documents and discs.**  
  Built in Rust · egui GUI · ffmpeg and yt-dlp included · Nothing to download after install

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
- **Zero setup** — ffmpeg, ffprobe and yt-dlp ship inside every package; FMX never downloads programs after install
- **Safe auto-updates** — every release file is Ed25519-signed; unsigned or tampered files are refused
- **Parallel queue** — jobs run side by side with live progress, cancel and retry
- **Light & dark theme** — follows system preference; smooth hover/press animations; "Reduce motion" option
- **5 languages** — English, Spanish, French, German and Hebrew, with a fully mirrored right-to-left layout for Hebrew (or set `FMX_LANG=xx`)
- **Drag & drop everywhere** — files, folders and links; open files directly from the command line
- **Right-click to convert** — "Convert with FMX" in Windows Explorer, Dolphin, Nemo, Nautilus and Caja turns a selection into MP4, MP3, JPG, PDF and more in one click (Settings → General)

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
A release is the **only** thing FMX ever downloads: the bundled ffmpeg, ffprobe and
yt-dlp are updated together with FMX, never on their own. Updates are safe by design:

- Only talks to this repository's GitHub releases, over HTTPS
- Never offers a downgrade, draft or pre-release
- Every file is verified against a minisign Ed25519 signature that names the exact file and version
- A file that fails the check is deleted and never run

Turn auto-check off, or hit **Check now**, in **Settings → Updates**.

---

## ⚙️ Requirements

**Nothing needs to be installed upfront.** Every release package bundles:

| Program | Source |
|---|---|
| **ffmpeg** + **ffprobe** | Pinned FFmpeg 8.1 shared build (Windows, Linux) or FFmpeg 6.1 static build (macOS, ffmpeg only), SHA-256 verified at build time |
| **yt-dlp** | Official standalone release (no Python needed), SHA-256 verified at build time |

They live in a `tools/` folder beside `fmx` (`/usr/lib/fmx` for the `.deb`), and FMX
always prefers them over anything on `PATH`. The shared ffmpeg build lets ffmpeg and
ffprobe reuse one set of libraries, so the bundle is about half the size of two static
programs. Licenses are listed in
[`packaging/THIRD-PARTY-NOTICES.md`](packaging/THIRD-PARTY-NOTICES.md).

Optional system programs that unlock extra tools when found next to the app or on `PATH`
(they are large desktop suites, so FMX uses your installed copy instead of bundling them):

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
# Fetch the same ffmpeg, ffprobe and yt-dlp a release ships, beside the dev binary
python3 packaging/fetch-bundled.py target/release/tools

# Run in development mode
cargo run --release

# Run the test suite
cargo test

# End-to-end tests (requires ffmpeg on PATH or FMX_FFMPEG set)
FMX_FFMPEG=/usr/bin/ffmpeg cargo test e2e -- --nocapture
```

Rust 1.89+ is required (`rust-version` in `Cargo.toml`).

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
| `FMX_LANG=es\|fr\|de\|he\|en` | Override UI language (also changeable in Settings) |
| `FMX_LOG=<filter>` | Log level filter, e.g. `debug` (overrides the Settings switch) |
| `FMX_FFMPEG=<path>` | Force a specific ffmpeg binary |
| `FMX_RENDERER=glow` | Windows: use OpenGL instead of Direct3D/Vulkan |

| Option | Effect |
|---|---|
| `fmx --debug` | Debug logging for this run, even if FMX can't open its window |
| `fmx --self-test` | Open the window, draw a few frames and quit (exit code 0 = FMX works here) |
| `fmx --convert <target> <files…>` | Convert right away, as the right-click menu does (`mp4`, `mkv`, `webm`, `gif`, `mp3`, `m4a`, `flac`, `wav`, `opus`, `jpg`, `png`, `webp`, `pdf`, `word`, `txt`, `images`) |
| `fmx --register-shell` / `--unregister-shell` | Add or remove the right-click menu for this user, then quit |

Only one FMX window runs at a time: starting FMX again (or converting from the right-click
menu) hands the files to the open window.

### Logs and crash reports

FMX writes `fmx.log` (and keeps the previous run's `fmx.previous.log`) in its logs folder:
`%APPDATA%\fmx\logs` on Windows, `~/.local/share/fmx/logs` on Linux and
`~/Library/Application Support/fmx/logs` on macOS. Turn on **Settings → General →
Debug logging** to record everything, and use **Open logs folder** to find the files. If FMX
crashes or can't start, it saves a `crash-<time>.txt` report there with the details and says so.

To add a language, add `locales/xx.json` and register it in `src/i18n.rs`; list it in
`RTL_LANGUAGES` there too if it is written right to left, and the whole UI mirrors.

---

## ⚠️ Known limitations

- **Copy-protected discs:** ffmpeg does not decrypt CSS (DVD) or AACS (Blu-ray).
- **Screen recorder on Wayland:** can only capture XWayland windows; log in to an X11 session to record the full desktop.
- **No built-in player:** finished files open in your default system player.

---

## 📄 License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) — your choice.
