#!/usr/bin/env python3
"""Download the programs FMX bundles into a tools directory for release packages.

Layout written to <tools-dir> (FMX looks for it beside its executable, or at
/usr/lib/fmx for the Debian package):

  bin/ffmpeg, bin/ffprobe, bin/yt-dlp   (.exe on Windows; DLLs sit in bin/)
  lib/libav*.so.N                       (Linux only; ffmpeg's RPATH is $ORIGIN/../lib)

Linux and Windows use BtbN's *shared* GPL builds: ffmpeg and ffprobe are tiny
launchers over one set of libraries, about half the size of two static
binaries. macOS uses eugeneware's static ffmpeg only; FMX reads media info
from ffmpeg's own output when ffprobe is absent, which saves ~120 MB in the
universal app. yt-dlp is the latest official standalone binary at build time,
checked against its release's SHA2-256SUMS, so each FMX release ships a fresh
copy that needs no Python.

Usage:
  packaging/fetch-bundled.py <tools-dir> [linux64|linuxarm64|win64|macos-universal]
"""

from __future__ import annotations

import gzip
import hashlib
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

BTBN = "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-25-13-00"
BTBN_BUILD = "ffmpeg-n8.1.3-24-g92611900bb"
EUGENEWARE = "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1"
YTDLP_LATEST = "https://github.com/yt-dlp/yt-dlp/releases/latest/download"

# Pinned archives; bump the URL and SHA-256 together when upgrading ffmpeg.
BTBN_SHARED = {
    "linux64": (
        f"{BTBN}/{BTBN_BUILD}-linux64-gpl-shared-8.1.tar.xz",
        "b0383286ca8e46de7c44bf678481794c8ddd35cf2e4f2f4dc8727f8b711c63f9",
    ),
    "linuxarm64": (
        f"{BTBN}/{BTBN_BUILD}-linuxarm64-gpl-shared-8.1.tar.xz",
        "69aaf04c75db9ae2c7779c0ced55301812f3e38ca9173c792ecb959681d37831",
    ),
    "win64": (
        f"{BTBN}/{BTBN_BUILD}-win64-gpl-shared-8.1.zip",
        "7416150e7dbab588c54e8aa38d4fa2f93adcfd8d0498a88c829e4ea236e442f5",
    ),
}
MACOS_FFMPEG = {
    "x64": (
        f"{EUGENEWARE}/ffmpeg-darwin-x64.gz",
        "929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106",
    ),
    "arm64": (
        f"{EUGENEWARE}/ffmpeg-darwin-arm64.gz",
        "8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa",
    ),
}
# Release asset -> installed name.
YTDLP = {
    "linux64": ("yt-dlp_linux", "yt-dlp"),
    "linuxarm64": ("yt-dlp_linux_aarch64", "yt-dlp"),
    "win64": ("yt-dlp.exe", "yt-dlp.exe"),
    "macos-universal": ("yt-dlp_macos", "yt-dlp"),
}

# Only the launchers FMX runs; ffplay and headers/docs are left out.
PROGRAMS = {"ffmpeg", "ffprobe", "ffmpeg.exe", "ffprobe.exe"}
SONAME = re.compile(r"^lib\w+\.so\.\d+$")
DLL = re.compile(r"^\w+-\d+\.dll$")


def detect_platform() -> str:
    system, machine = platform.system().lower(), platform.machine().lower()
    if system == "linux":
        return "linuxarm64" if machine in ("aarch64", "arm64") else "linux64"
    if system == "windows":
        return "win64"
    if system == "darwin":
        return "macos-universal"
    raise SystemExit(f"unsupported platform: {system}/{machine}")


def download(url: str, sha256: str | None = None) -> str:
    """Downloads `url` to a temporary file and checks its digest when given."""
    print(f"Downloading {url}")
    fd, path = tempfile.mkstemp()
    req = urllib.request.Request(url, headers={"User-Agent": "fmx-packaging"})
    digest = hashlib.sha256()
    try:
        with urllib.request.urlopen(req, timeout=300) as src, os.fdopen(fd, "wb") as out:
            for chunk in iter(lambda: src.read(1 << 20), b""):
                digest.update(chunk)
                out.write(chunk)
        if sha256 and digest.hexdigest() != sha256:
            raise SystemExit(f"digest mismatch for {url}: got {digest.hexdigest()}, want {sha256}")
    except BaseException:
        os.remove(path)
        raise
    return path


def write_exec(src, target: str) -> None:
    os.makedirs(os.path.dirname(target), exist_ok=True)
    with open(target, "wb") as out:
        shutil.copyfileobj(src, out)
    os.chmod(target, 0o755)


def extract_tar_shared(archive: str, tools: str) -> None:
    """bin/ffmpeg, bin/ffprobe, and each lib/libX.so.N (symlinks resolved)."""
    with tarfile.open(archive, "r:xz") as tar:
        members = {m.name: m for m in tar.getmembers()}
        for member in members.values():
            parent, name = os.path.split(member.name)
            if os.path.basename(parent) == "bin" and name in PROGRAMS and member.isfile():
                target = os.path.join(tools, "bin", name)
            elif os.path.basename(parent) == "lib" and SONAME.match(name):
                target = os.path.join(tools, "lib", name)
                while member.issym():
                    member = members[os.path.normpath(os.path.join(parent, member.linkname))]
            else:
                continue
            src = tar.extractfile(member)
            if src is None:
                raise SystemExit(f"cannot read {member.name}")
            with src:
                write_exec(src, target)


def extract_zip_shared(archive: str, tools: str) -> None:
    """bin/ffmpeg.exe, bin/ffprobe.exe and the DLLs they load."""
    with zipfile.ZipFile(archive) as zf:
        for info in zf.infolist():
            parent, name = os.path.split(info.filename)
            if os.path.basename(parent) == "bin" and (name in PROGRAMS or DLL.match(name)):
                with zf.open(info) as src:
                    write_exec(src, os.path.join(tools, "bin", name))


def fetch_ffmpeg(tools: str, platform_id: str) -> None:
    if platform_id == "macos-universal":
        thin = []
        try:
            for arch in ("x64", "arm64"):
                url, sha256 = MACOS_FFMPEG[arch]
                archive = download(url, sha256)
                fd, binary = tempfile.mkstemp()
                os.close(fd)
                thin.append(binary)
                with gzip.open(archive, "rb") as src, open(binary, "wb") as out:
                    shutil.copyfileobj(src, out)
                os.remove(archive)
            target = os.path.join(tools, "bin", "ffmpeg")
            os.makedirs(os.path.dirname(target), exist_ok=True)
            subprocess.check_call(["lipo", "-create", "-output", target, *thin])
            os.chmod(target, 0o755)
        finally:
            for path in thin:
                os.remove(path)
        return
    url, sha256 = BTBN_SHARED[platform_id]
    archive = download(url, sha256)
    try:
        if url.endswith(".zip"):
            extract_zip_shared(archive, tools)
        else:
            extract_tar_shared(archive, tools)
    finally:
        os.remove(archive)


def ytdlp_digest(asset: str) -> str:
    sums = download(f"{YTDLP_LATEST}/SHA2-256SUMS")
    try:
        with open(sums, encoding="utf-8") as f:
            for line in f:
                parts = line.split()
                if len(parts) >= 2 and parts[-1].lstrip("*") == asset and len(parts[0]) == 64:
                    return parts[0].lower()
    finally:
        os.remove(sums)
    raise SystemExit(f"{asset} not listed in SHA2-256SUMS")


def fetch_ytdlp(tools: str, platform_id: str) -> None:
    asset, name = YTDLP[platform_id]
    binary = download(f"{YTDLP_LATEST}/{asset}", ytdlp_digest(asset))
    try:
        with open(binary, "rb") as src:
            write_exec(src, os.path.join(tools, "bin", name))
    finally:
        os.remove(binary)


def main(argv: list[str]) -> None:
    if len(argv) < 2 or argv[1] in ("-h", "--help"):
        raise SystemExit(__doc__.strip())
    tools = os.path.abspath(argv[1])
    platform_id = argv[2] if len(argv) > 2 else detect_platform()
    if platform_id not in YTDLP:
        raise SystemExit(f"unknown platform {platform_id}; expected one of {', '.join(YTDLP)}")
    # Start clean so a library dropped by a newer build can't linger.
    for sub in ("bin", "lib"):
        shutil.rmtree(os.path.join(tools, sub), ignore_errors=True)
    fetch_ffmpeg(tools, platform_id)
    fetch_ytdlp(tools, platform_id)
    for dirpath, _, files in sorted(os.walk(tools)):
        for name in sorted(files):
            path = os.path.join(dirpath, name)
            print(f"  {os.path.getsize(path) / 1e6:7.1f} MB  {os.path.relpath(path, tools)}")


if __name__ == "__main__":
    main(sys.argv)
