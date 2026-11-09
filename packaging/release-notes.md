<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:dfd4f5e266a7a059abc3e3e41b42a4a26f3812870a9899bf50003256f22bcfbf -->

## What's new

### Photo and video slideshows
- Combine photos and video clips into a 1280 × 720 MP4, preserving video audio.
- Choose photo duration, output quality, and cut, fade, slide, or random transitions.

### Batch subtitle matching
- Match SRT, ASS, SSA, or VTT subtitles to videos by filename when adding subtitles in a batch.
- FMX reports missing or ambiguous matches before adding jobs to the queue.

### Security and reliability
- Limit resource use when processing large or malformed media and documents.
- Verify a protected copy of Debian updates before installation and harden desktop menu cleanup.
- Improve cancellation during CSO decompression and speed up output naming for large batches.



<!-- /coderabbit-ai-generated:v1 -->

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
