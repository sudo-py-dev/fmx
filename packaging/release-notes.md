## What's new

### Interface and preview
- Improve translated labels and accessible names for icon buttons.
- Show media details without blocking the interface.
- Improve the preview controls and keep exported rotations consistent with the preview.

### Capture and reliability
- Include local timestamps in screenshot and screen-recording filenames.
- Keep cancellation working when a child process closes its output stream.
- Ignore external yt-dlp configuration and improve handling of download metadata errors.
- Add a fast source and translation audit for developers.

### Smaller packages and faster releases
- Optimize the FMX executable for size across Linux, Windows and macOS.
- Increase compression for the universal Mac disk image.
- Improve release build caching and reuse the cached release-signing tool.
- Upload compressed packages without compressing them again.
- Report package sizes in the release workflow summary.

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
