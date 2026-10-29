## What's new

### A cleaner look, like GNOME
- Settings and tool search now sit at the top of the sidebar, so the tools get the whole window.
- An open tool keeps the sidebar and has a simple back arrow and title instead of a top bar.
- Tool cards show just the format's name, without repeating the file extension.

### Smoother motion
- Opening a tool slides its page in, and going back slides it away.
- The sidebar highlight glides to the category you pick, and the new page fades in.

### Also new
- See what changed in a new version right in Settings → Updates, before you update.

### Fixed
- Menus in Hebrew are no longer stretched across the window.
- Hebrew labels no longer run under the scroll bar.
- The Windows shortcut is now named "fmx".

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
