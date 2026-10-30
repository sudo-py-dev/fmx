#!/usr/bin/env python3
"""Install FMX's pinned FFmpeg and yt-dlp bundle from an FMX release asset.

The large programs live in an immutable release of this repository rather than
Git history. `packaging/vendor-tools.py` records and verifies the upstream
inputs used to maintain that release.

Usage:
  packaging/fetch-bundled.py <tools-dir> [linux64|win64|macos-universal]
"""

from __future__ import annotations

import hashlib
import os
import platform
import shutil
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

BUNDLE = (
    "https://github.com/sudo-py-dev/fmx/releases/download/"
    "tools-ffmpeg-9.0.2-ytdlp-2026.10.26"
)
ASSETS = {
    "linux64": (
        "fmx-tools-linux64.tar.xz",
        "a089994c9b0da1ae85317cc4e81ff75f39f4d6c66ee93fdbae906b4c5ea5a92f",
    ),
    "win64": (
        "fmx-tools-win64.zip",
        "109eef2f3905854ae0624af28d61d721bcd1a05cc7a5d8256a195716efa0a050",
    ),
    "macos-universal": (
        "fmx-tools-macos-universal.tar.xz",
        "34c1d120bb80d4e4f9340ada60df3b1c8bf82cff981ca255582cfe46ab3808f6",
    ),
}


def detect_platform() -> str:
    system, machine = platform.system().lower(), platform.machine().lower()
    if system == "linux" and machine in ("x86_64", "amd64"):
        return "linux64"
    if system == "windows" and machine in ("x86_64", "amd64"):
        return "win64"
    if system == "darwin":
        return "macos-universal"
    raise SystemExit(f"unsupported release platform: {system}/{machine}")


def download(url: str, sha256: str) -> str:
    print(f"Downloading FMX-owned bundle {url}")
    fd, path = tempfile.mkstemp()
    digest = hashlib.sha256()
    request = urllib.request.Request(url, headers={"User-Agent": "fmx-packaging"})
    try:
        with urllib.request.urlopen(request, timeout=300) as src, os.fdopen(fd, "wb") as out:
            for chunk in iter(lambda: src.read(1 << 20), b""):
                digest.update(chunk)
                out.write(chunk)
        if digest.hexdigest() != sha256:
            raise SystemExit(
                f"digest mismatch for {url}: got {digest.hexdigest()}, want {sha256}"
            )
    except BaseException:
        os.remove(path)
        raise
    return path


def extract(archive: str, name: str, tools: str) -> None:
    if name.endswith(".zip"):
        with zipfile.ZipFile(archive) as zf:
            for member in zf.namelist():
                parts = member.replace("\\", "/").split("/")
                if member.startswith(("/", "\\")) or ".." in parts:
                    raise SystemExit(f"unsafe path in FMX tool bundle: {member}")
            zf.extractall(tools)
    else:
        with tarfile.open(archive, "r:xz") as tar:
            tar.extractall(tools, filter="data")


def main(argv: list[str]) -> None:
    if len(argv) < 2 or argv[1] in ("-h", "--help"):
        raise SystemExit(__doc__.strip())
    tools = os.path.abspath(argv[1])
    platform_id = argv[2] if len(argv) > 2 else detect_platform()
    if platform_id not in ASSETS:
        raise SystemExit(f"unknown platform {platform_id}; expected one of {', '.join(ASSETS)}")
    for subdir in ("bin", "lib"):
        shutil.rmtree(os.path.join(tools, subdir), ignore_errors=True)
    name, sha256 = ASSETS[platform_id]
    archive = download(f"{BUNDLE}/{name}", sha256)
    try:
        extract(archive, name, tools)
    finally:
        os.remove(archive)
    for dirpath, _, files in sorted(os.walk(tools)):
        for filename in sorted(files):
            path = os.path.join(dirpath, filename)
            print(f"  {os.path.getsize(path) / 1e6:7.1f} MB  {os.path.relpath(path, tools)}")


if __name__ == "__main__":
    main(sys.argv)
