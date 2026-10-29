## What's new

### Screen recorder and screenshots
- Records on Wayland (GNOME, KDE) through the system's screen-sharing dialog.
- Choose the sound: none, microphone, computer sound, or both; pick a monitor or a custom area, and hide the mouse pointer.
- Save as MP4, MKV, WebM or MOV, with quality presets, a 3-second countdown, and an option to minimize FMX first.
- New Screenshot tool: the whole screen, a monitor or an area, as PNG or JPEG, after a delay.

### Video preview
- Play and pause the video (Space), skip with the buttons or arrow keys; the picture is bigger and centred.
- Trim has one simple start/end row under the video.
- Extract Frames can save frames at moments you pick while watching, every frame, or every few seconds from part of a video.
- Rotate has turn and flip buttons under the picture that combine, with a live preview.

### Simpler and cleaner
- Tool options are tidy rows split by lines, with switches for on/off options.
- The queue is a clean list: one row per job, long names shortened, actions at the end.
- Settings groups have titles, and "When done" moved there from the queue.
- CD to Audio lists your disc drives instead of asking for a path.
- YouTube downloads offer only the qualities the video has.

### Fixed
- The System theme follows the desktop's light/dark change without restarting.

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
