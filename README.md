# FMX — Free Media Xpress

<img src="packaging/icons/fmx-128.png" alt="FMX icon" width="80" align="right" />

Convert video, audio, images, documents and discs in one desktop app for Windows,
macOS and Linux. FMX combines format presets, editing tools and a batch queue,
with bundled FFmpeg and yt-dlp for common conversion and download tasks.

[**Download the latest release**](https://github.com/sudo-py-dev/fmx/releases/latest)
· [Getting started](#your-first-conversion)
· [Build and contribute](DEVELOPMENT.md)
· [Report a problem](https://github.com/sudo-py-dev/fmx/issues)

[![Release](https://img.shields.io/github/v/release/sudo-py-dev/fmx?style=flat-square&color=3584e4)](https://github.com/sudo-py-dev/fmx/releases/latest)
[![License: MIT or Apache-2.0](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue?style=flat-square)](#license)

![FMX main window](assets/screenshot.png)

- Queue files or folders and process several jobs in parallel.
- Use device presets, or adjust output settings and edit before converting.
- Find tools with search and pin frequently used tools to Favorites.
- Convert from the file manager's right-click menu on Windows and Linux.
- Choose light or dark mode and one of six languages: English, Español, Français,
  Deutsch, עברית or Русский, with a mirrored layout for Hebrew.
- Free and open source, with no ads or account required.

## Download and install

Get the file for your computer from the [**latest release**](https://github.com/sudo-py-dev/fmx/releases/latest):

| Your computer | Download | Then |
|---|---|---|
| **Windows 10 / 11** (64-bit) | `…-windows-x64-setup.exe` | Double-click it and follow the steps. |
| **macOS 11+** (Intel or Apple silicon) | `…-macos-universal.dmg` | Open it and drag FMX to Applications. |
| **Ubuntu / Debian** | `…_amd64.deb` | Double-click it to install, or run `sudo apt install ./fmx_*_amd64.deb` |
| **Other Linux** (x86-64) | `…-linux-x86_64.tar.gz` | Unpack it, open the extracted folder, and run `./fmx`. Keep the `tools/` folder beside it. |

> **Windows says "Windows protected your PC"?** Click **More info → Run anyway**. The installer may trigger an unknown-publisher warning.
>
> **Mac says FMX "can't be opened"?** Right-click FMX in Applications and choose **Open** the first time.

---

## Your first conversion

1. **Pick what you want to make** - choose a category on the left (Video, Audio, Image…) and click a format, like **MP4** or **MP3**.
2. **Add your files** - drag them into the window, or click **Add files**.
3. **Choose the output folder**, then click **Add to queue**. Jobs start automatically when that setting is enabled; otherwise, click **Start** in the queue.

The default settings work well for most people. If you want more control, open **Output settings** or **Edit** to trim, crop, rotate or add subtitles.

<div align="center">

![Converting a video in FMX](assets/screenshot-editor.png)

</div>

When everything is done, FMX can play a sound, show a notification, open the folder - or even shut down your computer.

---

## Conversion and editing tools

| Category | What you can do |
|---|---|
| 🎬 **Video** | Convert to MP4, MKV, WebM, MOV, AVI, GIF and more. Trim, crop, rotate, add subtitles or a watermark. |
| 📱 **Phones & devices** | Ready-made settings for iPhone, iPad, Android, smart TVs, YouTube, WhatsApp, PlayStation and Xbox. |
| 🎵 **Audio** | Convert to MP3, M4A, FLAC, WAV, OGG and more. Pull the sound out of any video. Make iPhone ringtones. |
| 🖼️ **Pictures** | Convert to JPG, PNG, WebP and more. Resize, rotate, make black-and-white. Opens iPhone (HEIC) photos. |
| 📄 **Documents** | Pictures → PDF, PDF → Word, Excel or pictures, Word/Excel/PowerPoint → PDF. Merge, split and shrink PDFs. Convert eBooks. |
| 💿 **DVD / CD** | Copy DVDs and Blu-rays to video files, rip music CDs, and make or open ISO disc images. |
| 🔧 **Extras** | Join videos or songs, make a video smaller, change speed or volume, remove a logo, download videos from a link, record your screen, or record microphone and system audio. |

<details>
<summary><b>See every supported format</b></summary>

**Video:** MP4 (H.264 · H.265) · MKV · WebM · MOV · AVI · FLV · WMV · MPG · VOB · TS · OGV · 3GP · SWF · GIF · Animated WebP

**Audio:** MP3 · AAC · M4A · M4R ringtone · FLAC · WAV · OGG · Opus · WMA · AIFF · AC3 · MP2 · WavPack · MKA · MMF · AMR · APE · ALAC (Apple Lossless) · CAF · AU

**Image:** PNG · JPG · WebP · BMP · TIFF · GIF · ICO · TGA · QOI · AVIF · JPEG XL · PCX · PPM · PGM · PAM

**Documents:** PDF · Word · Excel · TXT · HTML · EPUB · FB2 · MOBI / AZW3 (with Calibre)

**Discs:** DVD · Blu-ray · Music CD · ISO · CSO

</details>

---

## Video downloader

Paste a video, playlist or channel link from YouTube, X (Twitter), Vimeo, TikTok,
or another [site supported by yt-dlp](https://github.com/yt-dlp/yt-dlp/blob/master/supportedsites.md).
Site support depends on the bundled yt-dlp version and the site's current restrictions.

The Download selector offers video, audio, subtitles only, thumbnail only, and
metadata with a text description. Video and audio retain the existing quality,
format, cover-art, metadata and speed-limit controls.

Open **More download options** for:

- Playlist indices and slices (`1:5,8` selects items 1–5 and 8).
- A clip's start and end in seconds, or an open-ended range.
- Separate files for the video's chapters, while retaining the complete download.
- SponsorBlock removal of sponsored segments using community timestamps.
- A download history in the output folder to skip completed media. Histories are
  separated by media kind, format, quality, clip range and chapter/sponsor choices.
- Live streams from the beginning, where supported. Recording continues until
  the stream finishes or the job is cancelled.
- Automatic captions and separate subtitle, thumbnail, JSON metadata and description files.
- Parallel DASH/HLS fragments and retry counts.
- A Netscape-format cookies file for signed-in access; the same file is used
  for the link preview. Clear the field to stop using it.

The interface covers these media tasks rather than exposing every yt-dlp command-line
switch. Custom commands/plugins, arbitrary output templates, proxy and extractor
configuration, credential entry, and unattended channel scheduling remain CLI workflows.

---

## Convert with a right-click

Select one or more files in your file manager, right-click, and choose **Convert with FMX**:

| Files | You can turn them into |
|---|---|
| Videos | MP4 · MKV · WebM · GIF · MP3 (just the sound) |
| Music | MP3 · M4A · FLAC · WAV · Opus |
| Pictures | JPG · PNG · WebP · PDF (all pictures in one PDF) |
| Word, Excel, PowerPoint | PDF |
| eBooks | PDF · TXT · EPUB · FB2 |
| PDF | Word · TXT · Pictures |

FMX starts right away and saves the new files next to the originals. Choose **Open in FMX** instead if you want to change the settings first.

- **Windows 11:** it's under **Show more options**.
- **Linux:** in Dolphin and Nemo it's in the menu; in Files (Nautilus) and Caja it's under **Scripts**.
- Turn it on or off any time in **Settings → General → Right-click menu**. (Not available on Mac.)

---

## Appearance

<div align="center">

![FMX in dark mode](assets/screenshot-dark.png)

</div>

Open **Settings → General → Appearance** and pick **System**, **Light** or **Dark**. If animations bother you, turn on **Reduce motion**.

---

## Updates

FMX checks for a new version once a day. When one is ready, a small blue dot appears on **Settings** - open **Settings → Updates** and click **Update now**. Nothing pops up while you work.

Every update is digitally signed, so FMX only installs genuine releases from this page.

---

## Optional programs

Release packages bundle FFmpeg and yt-dlp. Some formats and document or disc tools
need an additional program. FMX detects available programs and disables tools whose
requirements are missing; supported codecs also depend on the FFmpeg build.

| To… | Install |
|---|---|
| Turn PDFs into pictures, Word, Excel or text, or merge/split PDFs | Poppler (`poppler-utils`) |
| Make PDFs smaller | Ghostscript |
| Convert Kindle books (MOBI, AZW3) | Calibre |
| Turn a folder into an ISO | xorriso |
| Convert old `.doc` / `.ppt` files | LibreOffice |
| Create APE audio files | Monkey’s Audio |

You can check what's installed in **Settings → Programs**.

---

## Troubleshooting

1. Open **Settings → General** and turn on **Debug logging**.
2. Try the same thing again.
3. Click **Open logs folder** and attach the files when you [report the problem](https://github.com/sudo-py-dev/fmx/issues).

If FMX closes unexpectedly, it saves a crash report in the same folder and tells you about it the next time it starts.

**Good to know**

- Copy-protected DVDs and Blu-rays can't be converted.
- On Linux with Wayland, screen capture uses the desktop portal: choose the screen or window in the system sharing dialog.
- On Windows and Mac, choose your language in **Settings → General** (it isn't detected automatically yet).
- On Ubuntu, turn off the right-click menu in Settings *before* uninstalling FMX.

---

## License

FMX is free to use, share and change, under the [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) license.

Want to build FMX yourself or help out? See [**DEVELOPMENT.md**](DEVELOPMENT.md).
