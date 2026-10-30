#!/usr/bin/env python3
"""Build an FMX-owned tool bundle from pinned, checksum-verified upstream files.

Layout written to <tools-dir> (FMX looks for it beside its executable, or at
/usr/lib/fmx for the Debian package):

  bin/ffmpeg, bin/ffprobe, bin/yt-dlp   (.exe on Windows; DLLs sit in bin/)
  lib/libav*.so.N                       (Linux only; ffmpeg's RPATH is $ORIGIN/../lib)

Linux and Windows use BtbN's *shared* GPL builds: ffmpeg and ffprobe are tiny
launchers over one set of libraries, about half the size of two static
binaries. macOS uses Martin Riedl's static ffmpeg only; FMX reads media info
from ffmpeg's own output when ffprobe is absent, which avoids adding a second
large universal binary. yt-dlp is the latest official standalone binary at
build time, checked against its release's SHA2-256SUMS, so each FMX release
ships a fresh copy that needs no Python.

Usage:
  packaging/fetch-bundled.py <tools-dir> [linux64|linuxarm64|win64|macos-universal]
"""

from __future__ import annotations

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

BTBN = "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-29-19-17"
BTBN_BUILD = "ffmpeg-n9.0.2-37-gae19868753"
RIEDL = "https://ffmpeg.martin-riedl.de/download/macos"
YTDLP_RELEASE = "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26"

# Pinned archives; bump the URL and SHA-256 together when upgrading ffmpeg.
BTBN_SHARED = {
    "linux64": (
        f"{BTBN}/{BTBN_BUILD}-linux64-gpl-shared-9.0.tar.xz",
        "9ada35cc8b27dd2181046f44351236742e2b71fa47b1599241192507d627ca83",
    ),
    "linuxarm64": (
        f"{BTBN}/{BTBN_BUILD}-linuxarm64-gpl-shared-9.0.tar.xz",
        "6fd63d0e7a1340d09c985bc70ba754a6e936b5de99ea00e587c41ddb70a68c9c",
    ),
    "win64": (
        f"{BTBN}/{BTBN_BUILD}-win64-gpl-shared-9.0.zip",
        "6cc0927ed9ed1c97cc5286dbabc5aea376de456b7d117036392db7fd0a2c4f0b",
    ),
}
MACOS_FFMPEG = {
    "x64": (
        f"{RIEDL}/amd64/1789931006_9.0.2/ffmpeg.zip",
        "7c6b4125b191cbf773832dc51f424cf2b6bb7da43007d1e066f95909e47cacd4",
    ),
    "arm64": (
        f"{RIEDL}/arm64/1789931890_9.0.2/ffmpeg.zip",
        "c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924",
    ),
}
# Release asset -> installed name.
YTDLP = {
    "linux64": (
        "yt-dlp_linux",
        "yt-dlp",
        "767ba9dce7052aa196f8b12652f433d76de128ba168966fa84b6394d5e6c6795",
    ),
    "linuxarm64": (
        "yt-dlp_linux_aarch64",
        "yt-dlp",
        "e87e9533e89fe5889abaa21f5114dfe7756d6aa6219b83551add04d48b18e483",
    ),
    "win64": (
        "yt-dlp.exe",
        "yt-dlp.exe",
        "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d",
    ),
    "macos-universal": (
        "yt-dlp_macos",
        "yt-dlp",
        "0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa",
    ),
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
                with zipfile.ZipFile(archive) as zf:
                    matches = [
                        name
                        for name in zf.namelist()
                        if name.rstrip("/").endswith("/ffmpeg") or name == "ffmpeg"
                    ]
                    if len(matches) != 1:
                        raise SystemExit(f"expected one ffmpeg binary in {url}, found {matches}")
                    with zf.open(matches[0]) as src, open(binary, "wb") as out:
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


def fetch_ytdlp(tools: str, platform_id: str) -> None:
    asset, name, sha256 = YTDLP[platform_id]
    binary = download(f"{YTDLP_RELEASE}/{asset}", sha256)
    try:
        with open(binary, "rb") as src:
            write_exec(src, os.path.join(tools, "bin", name))
    finally:
        os.remove(binary)


def validate_bundle(tools: str, platform_id: str) -> None:
    """Run the owned programs before publishing an immutable bundle."""
    suffix = ".exe" if platform_id == "win64" else ""
    ffmpeg = os.path.join(tools, "bin", f"ffmpeg{suffix}")
    ytdlp = os.path.join(tools, "bin", f"yt-dlp{suffix}")
    ffmpeg_version = subprocess.check_output(
        [ffmpeg, "-version"], text=True, stderr=subprocess.STDOUT
    ).splitlines()[0]
    ytdlp_version = subprocess.check_output(
        [ytdlp, "--version"], text=True, stderr=subprocess.STDOUT
    ).strip()
    if "9.0.2" not in ffmpeg_version:
        raise SystemExit(f"unexpected FFmpeg version: {ffmpeg_version}")
    if ytdlp_version != "2026.10.26":
        raise SystemExit(f"unexpected yt-dlp version: {ytdlp_version}")
    if platform_id != "macos-universal":
        ffprobe = os.path.join(tools, "bin", f"ffprobe{suffix}")
        subprocess.check_call(
            [ffprobe, "-version"], stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT
        )
    print(f"Verified {ffmpeg_version}")
    print(f"Verified yt-dlp {ytdlp_version}")


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
    validate_bundle(tools, platform_id)
    for dirpath, _, files in sorted(os.walk(tools)):
        for name in sorted(files):
            path = os.path.join(dirpath, name)
            print(f"  {os.path.getsize(path) / 1e6:7.1f} MB  {os.path.relpath(path, tools)}")


if __name__ == "__main__":
    main(sys.argv)
