#!/usr/bin/env python3
"""Draws FMX's app icon (the blue clapperboard tile from the app) in every
size the installers need. Run from the repository root:

    python3 packaging/make-icons.py

Needs Pillow. Writes packaging/icons/: fmx.png (512 px), fmx-<n>.png,
fmx.ico (Windows) and fmx.icns (macOS).
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
FONT = ROOT / "assets" / "fonts" / "Phosphor-Fill.ttf"
OUT = ROOT / "packaging" / "icons"
FILM_SLATE = ""  # art::glyph::FILM_SLATE, the app's logo glyph
BLUE = (59, 130, 246)  # art::hue::BLUE
BASE = 1024
PNG_SIZES = [16, 24, 32, 48, 64, 128, 256, 512]


def draw(size: int) -> Image.Image:
    """The logo tile at 4x, scaled down for clean edges."""
    big = size * 4
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    pad = round(big * 0.06)
    box = (pad, pad, big - pad, big - pad)
    radius = round((big - 2 * pad) * 0.24)
    d.rounded_rectangle(box, radius=radius, fill=BLUE + (255,))
    # A soft top highlight, like the in-app solid tiles.
    top = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    ImageDraw.Draw(top).rounded_rectangle(box, radius=radius, fill=(255, 255, 255, 22))
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).rectangle((0, 0, big, big // 2), fill=255)
    img.paste(top, (0, 0), Image.composite(top, Image.new("RGBA", top.size), mask).getchannel("A"))
    font = ImageFont.truetype(str(FONT), round((big - 2 * pad) * 0.58))
    d = ImageDraw.Draw(img)
    d.text((big / 2, big / 2), FILM_SLATE, font=font, fill="white", anchor="mm")
    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    base = draw(BASE)
    for n in PNG_SIZES:
        draw(n).save(OUT / f"fmx-{n}.png")
    draw(512).save(OUT / "fmx.png")
    base.save(OUT / "fmx.ico", sizes=[(n, n) for n in [16, 24, 32, 48, 64, 128, 256]])
    base.save(OUT / "fmx.icns")
    print(f"icons written to {OUT}")


if __name__ == "__main__":
    main()
