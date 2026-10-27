# Third-party programs bundled with FMX

FMX itself is dual-licensed under MIT or Apache 2.0. Release packages also
ship the following unmodified programs in a `tools/` folder next to the FMX
executable (`/usr/lib/fmx` in the Debian package). FMX runs
them as separate processes and does not link to them. They are updated only
with a new FMX release; FMX never downloads programs after installation.

## FFmpeg (`ffmpeg`, `ffprobe`)

- Linux and Windows: shared GPL builds (programs plus the `libav*` /
  `av*.dll` libraries in `tools/`) from
  [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) (FFmpeg 8.1).
- macOS: static `ffmpeg` from
  [eugeneware/ffmpeg-static](https://github.com/eugeneware/ffmpeg-static) (FFmpeg 6.1).
- License: GNU General Public License v3 or later, as configured by the
  build. Source code: <https://ffmpeg.org/download.html>, with build scripts
  and the exact source revisions published on the projects above.

FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project.

## yt-dlp (`yt-dlp`)

- Official standalone release from <https://github.com/yt-dlp/yt-dlp>.
- License: The Unlicense (public domain). The standalone executable embeds
  Python and other components under their own licenses, listed in
  <https://github.com/yt-dlp/yt-dlp/blob/master/THIRD_PARTY_LICENSES.txt>.

Exact download URLs and SHA-256 checksums are pinned in
`packaging/fetch-bundled.py` in the FMX source repository.
