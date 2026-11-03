## What's new

### Saved presets
- Save a tool with its current settings as a named preset.
- Run presets from Favorites, the presets button, or a file's right-click menu.
- Start a preset from the command line with `--preset <id>`.

### Lossy WebP output
- Export lossy WebP images, keeping transparency.

### Smoother screen recording
- The timer shows the real elapsed time instead of lagging behind.
- On Wayland, recordings keep their real length and stay in sync with the sound.
- A "Dropping frames" warning appears when the recording falls well below the chosen frame rate.

### Fixes
- Collage layout thumbnails no longer overlap in right-to-left languages, and the editor fits in short windows.
- Desktop shortcuts keep working after FMX is updated or replaced.
- Previews use a bounded amount of memory.
- Queue rows show the full running status on hover.

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
