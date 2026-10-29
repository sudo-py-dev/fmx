# Developing FMX

Notes for building, testing and releasing FMX. For using the app, see the [README](README.md).

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

# Hear the three built-in completion sounds
cargo test plays_each_built_in_sound -- --ignored

# Check that FMX opens its window here (exit code 0 = it works)
cargo run --release -- --self-test
```

Rust 1.89+ is required (`rust-version` in `Cargo.toml`). Building on Linux needs no extra `-dev`
packages (the release workflow builds on a stock Ubuntu runner).

Debug builds can save a picture of the real window, to check layouts without clicking through
them: `FMX_SCREENSHOT=shot.png cargo run` (see `src/devshot.rs` for the switches that open
Settings, the editor, a menu and so on).

### Project layout

| Path | What it holds |
|---|---|
| `src/app.rs` | The window: tools, editor, queue and Settings |
| `src/catalog.rs`, `src/plan.rs`, `src/jobs.rs`, `src/exec.rs` | Which tools exist, and how a tool and files become ffmpeg (or native) jobs |
| `src/shell.rs`, `src/instance.rs` | The right-click menu, and handing files to the one running window |
| `src/update.rs` | Signed self-updates |
| `src/diagnostics.rs`, `src/sound.rs` | Logs and crash reports; the completion sound |
| `src/i18n.rs`, `src/dir.rs`, `locales/` | Translations, and the mirrored right-to-left layout |
| `vendor/epaint` | egui's text library with right-to-left text support ([FMX-PATCH.md](vendor/epaint/FMX-PATCH.md)) |
| `packaging/` | Bundled-tool fetching, installers and packages for each system |

### Adding a language

Add `locales/xx.json` (the English file lists every string) and register it in `src/i18n.rs`.
If the language is written right to left, list it in `RTL_LANGUAGES` there too, and the whole UI
mirrors.

---

## 📦 Bundled programs

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
| Calibre | Kindle eBooks (MOBI, AZW3), and reading MOBI/PDF books | `calibre` |
| xorriso | Folder → ISO | `xorriso` |
| Monkey's Audio | APE output | `monkeys-audio` |
| LibreOffice | Legacy `.doc`/`.ppt` → PDF | `libreoffice` |

> Formats unsupported by your ffmpeg build (e.g. AMR without `libopencore_amrnb`) are greyed out automatically.

On Linux the completion sound plays through `pw-play`, `paplay` or `aplay`, one of which every
desktop has; FMX itself needs no audio libraries.

---

## 🔐 How updates are checked

FMX checks GitHub once a day. When a newer release is out, a dot appears on the **Settings**
button, and **Settings → Updates** shows it with **Update now**, **What's new** and
**Skip this version**; nothing pops up over your work. A release is the **only** thing FMX
ever downloads: the bundled ffmpeg, ffprobe and yt-dlp are updated together with FMX, never
on their own. Updates are safe by design:

- Only talks to this repository's GitHub releases, over HTTPS
- Never offers a downgrade, draft or pre-release
- Every file is verified against a minisign Ed25519 signature that names the exact file and version
- A file that fails the check is deleted and never run

Turn auto-check off, or hit **Check now**, in **Settings → Updates**.

---

## 🚀 Making a release

1. Bump `version` in `Cargo.toml` (the only place — installers and the updater read it from there).
2. Tag and push, with the tag matching that version:
   ```bash
   git tag v1.2.3 && git push origin master v1.2.3
   ```
3. The [release workflow](.github/workflows/release.yml) builds on Linux, Windows and macOS.
   On Windows it also starts the real `fmx.exe` (`--self-test`, on a machine without a GPU) and
   adds and removes the right-click menu; if either fails, nothing is published. It then signs
   every file with `FMX_SIGNING_KEY`, verifies it against `packaging/update-key.pub`, and
   publishes the release.

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
| `fmx --convert <target> <files…>` | Convert right away, as the right-click menu does (`mp4`, `mkv`, `webm`, `gif`, `mp3`, `m4a`, `flac`, `wav`, `opus`, `jpg`, `png`, `webp`, `pdf`, `word`, `txt`, `images`, `epub`, `fb2`) |
| `fmx --register-shell` / `--unregister-shell` | Add or remove the right-click menu for this user, then quit |

Only one FMX window runs at a time: starting FMX again (or converting from the right-click
menu) hands the files to the open window.

---

## 🩺 Logs and crash reports

FMX writes `fmx.log` (and keeps the previous run's `fmx.previous.log`) in its logs folder:

| System | Logs folder |
|---|---|
| Windows | `%APPDATA%\fmx\logs` |
| Linux | `~/.local/share/fmx/logs` |
| macOS | `~/Library/Application Support/fmx/logs` |

If FMX crashes or can't start, it saves a `crash-<time>.txt` report (what happened, where, a
backtrace, the version and the system). If the window won't open at all, start it once with
`fmx --debug`.
