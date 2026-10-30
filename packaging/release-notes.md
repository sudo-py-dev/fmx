## What's new

### Favorites
- Star any tool to pin it. A Favorites entry at the top of the sidebar lists your starred tools, and FMX remembers them.

### Sound recorder
- New tool that records your microphone, the computer's sound, or both, as MP3, WAV, FLAC, OGG, Opus or M4A.

### Sidebar and search
- Each category shows how many tools it has.
- Search finds tools that match every word you type, and has a button to clear it.

### Hebrew and right-to-left text
- The text cursor and mouse clicks land in the right place in Hebrew and mixed Hebrew/English text.
- Clearer Hebrew wording across the app.

### Reliability
- After an update, the new FMX waits for the old window to close instead of quitting.
- Sliders have round handles that are easier to see.

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
