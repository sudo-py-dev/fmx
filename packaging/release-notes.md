## Download

| System | File |
|---|---|
| Windows 10/11 (64-bit) | `muxr-@VERSION@-windows-x64-setup.exe` |
| macOS 11+ (Intel and Apple silicon) | `muxr-@VERSION@-macos-universal.dmg` |
| Ubuntu / Debian (64-bit) | `muxr_@VERSION@_amd64.deb` |
| Other Linux (64-bit, portable) | `muxr-@VERSION@-linux-x86_64.tar.gz` |

**macOS:** Muxr is not notarized by Apple yet. The first time, right-click Muxr in
Applications and choose **Open**.

**Windows:** SmartScreen may warn about an unknown publisher; choose **More info → Run anyway**.

## Verify a download

Every file has a `.minisig` signature made with Muxr's release key, and `SHA256SUMS` lists
their checksums. Muxr's built-in updater checks the signature before installing anything.
To check a file yourself with [minisign](https://jedisct1.github.io/minisign/):

```
minisign -Vm <file> -P RWRGUcjat1hYfPTyJvzyamrOJ9OvHe2D7nxY3Pmah/5llIVcxpus8tCN
```
