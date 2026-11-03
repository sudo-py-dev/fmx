## What's new

### Custom window controls and navigation
- Replace the system title bar with a compact, theme-aware bar with custom close, minimize, maximize and restore controls.
- Mirror the controls for RTL/LTR languages, with translated tooltips, keyboard focus and hover feedback.
- Drag the title bar to move the window, double-click to maximize or restore, and resize from the edges and corners.
- Organize all 121 tools into task groups, improve search and Hebrew wording, and fix tool-card wrapping in both layout directions.
- Fix sidebar search layout when typing and clearing; move the usage guide to the Settings sidebar footer.

### Automatic language selection
- Detect the system display language on Windows and macOS and the locale on Linux, with English fallback for unsupported languages.
- Automatically localize the Windows installer in English, Spanish, French, German, Hebrew or Russian, falling back to English.
- Preserve an explicit language chosen in FMX Settings.

### Live image collages
- Create collages with two-column, three-column and spotlight templates.
- Preview photos live while changing spacing and photo fit.
- Click two photos to swap positions, then export a PNG at up to 4096 × 4096 pixels.
- Cache photo previews for responsive editing and keep the canvas centered in both layout directions.

### Media tools and metadata
- Add video reversal, fades, stabilization, denoising and sharpening.
- Add audio denoising, silence trimming and configurable vocal reduction for karaoke.
- Create video thumbnail sheets and waveform videos.
- Inspect media information without modifying files and remove photo metadata with native processing.
- Edit tags per file with scoped metadata and verify saved changes.

### Safer conversions and shared actions
- Stage conversion outputs privately and publish completed files without overwriting existing destinations.
- Clean up private temporary outputs after failures or cancellation while preserving destination data.
- Improve external-process handling and streamline shared tool actions and conversion helpers.
- Add per-tool desktop shortcuts that open the selected tool in FMX.
- Update the README for the expanded toolset and navigation, and switch repository branch references to main.

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
