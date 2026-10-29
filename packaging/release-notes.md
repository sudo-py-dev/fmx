## What's new

### A cleaner Settings window
- Every setting is now one tidy row: its name and a short description on one side, its control on the other.
- On/off settings use switches, and the long explanations moved into tooltips.

### New About page
- Settings → About shows the version, links to the website, problem reports and all releases, and FMX's licenses.

### Fixed
- Tool pages no longer jump sideways when you open a section like Edit or Output settings.

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
