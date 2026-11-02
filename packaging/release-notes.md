## What's new

### Getting started
- A welcome guide walks through the basics on first launch.
- Russian is now available alongside the existing interface languages.

### More conversion formats
- Added ALAC (Apple Lossless), CAF and AU audio output.
- Added PPM, PGM and PAM image output.

### Video downloader
- Save video, audio, subtitles, thumbnails or metadata.
- Choose playlist items and clip ranges, split chapters and use SponsorBlock.
- Added download history, live-stream options and cookies-file support.

### Documentation and checks
- Clearer setup instructions and new screenshots in the README and download website.
- macOS lint and test checks now run on code changes, alongside Linux and Windows.

## Download

| System | File |
|---|---|
| Windows 10/11 (64-bit) | `fmx-@VERSION@-windows-x64-setup.exe` |
| macOS 11+ (Intel and Apple silicon) | `fmx-@VERSION@-macos-universal.dmg` |
| Ubuntu / Debian (64-bit) | `fmx_@VERSION@_amd64.deb` |
| Other Linux (64-bit, portable) | `fmx-@VERSION@-linux-x86_64.tar.gz` |

**macOS:** FMX is not notarized by Apple yet. The first time, right-click FMX in
Applications and choose **Open**.

**Windows:** SmartScreen may warn about an unknown publisher; choose **More info → Run anyway**.

## Verify a download

Every file has a `.minisig` signature made with FMX's release key, and `SHA256SUMS` lists
their checksums. FMX's built-in updater checks the signature before installing anything.
To check a file yourself with [minisign](https://jedisct1.github.io/minisign/):

```
minisign -Vm <file> -P RWRGUcjat1hYfPTyJvzyamrOJ9OvHe2D7nxY3Pmah/5llIVcxpus8tCN
```
