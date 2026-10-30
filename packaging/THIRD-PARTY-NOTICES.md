# Third-party components in FMX

FMX itself is dual-licensed under MIT or Apache 2.0. Release packages also
ship FFmpeg and yt-dlp, unmodified, in a `tools/` folder next to the FMX
executable (`/usr/lib/fmx` in the Debian package). FMX runs them as separate
processes and does not link to them. They are updated only with a new FMX
release; FMX never downloads programs after installation. The last two
entries below are compiled into the FMX executable itself.

## FFmpeg (`ffmpeg`, `ffprobe`)

- Linux and Windows: shared GPL builds (programs plus the `libav*` /
  `av*.dll` libraries in `tools/`) based on
  [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) (FFmpeg 9.0.2).
- macOS: static `ffmpeg` from
  [Martin Riedl's FFmpeg Build Server](https://ffmpeg.martin-riedl.de/) (FFmpeg 9.0.2).
- License: GNU General Public License v3 or later, as configured by the
  build. Source code: <https://ffmpeg.org/download.html>, with build scripts
  and the exact source revisions published on the projects above.

FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project.

## yt-dlp (`yt-dlp`)

- Official standalone release from <https://github.com/yt-dlp/yt-dlp>.
- License: The Unlicense (public domain). The standalone executable embeds
  Python and other components under their own licenses, listed in
  <https://github.com/yt-dlp/yt-dlp/blob/master/THIRD_PARTY_LICENSES.txt>.

## Noto Sans Hebrew (built into `fmx`)

- From <https://github.com/notofonts/hebrew> (release NotoSansHebrew-v3.001).
- License: SIL Open Font License 1.1, see `assets/fonts/NotoSansHebrew-OFL.txt`
  in the FMX source repository.

## epaint (built into `fmx`)

- egui's text and painting crate, <https://github.com/emilk/egui>, version
  0.37.0, with right-to-left text support added by FMX
  (`vendor/epaint` in the FMX source repository).
- License: MIT OR Apache-2.0.

FMX keeps reduced, immutable copies of these programs in its own GitHub
release assets. Exact upstream URLs and SHA-256 checksums are pinned in
`packaging/vendor-tools.py`; release packages download only the FMX-owned,
checksum-pinned bundle recorded in `packaging/fetch-bundled.py`.
