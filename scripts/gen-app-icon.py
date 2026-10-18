#!/usr/bin/env python3
"""Regenerate the qrtz desktop app icons (apps/desktop/src-tauri/icons).

macOS Big Sur+ template: an 824x824 rounded-rect dark tile (corner radius 185)
centred on a transparent 1024x1024 canvas (100 px margin) with a subtle drop
shadow, faint gradient and hairline rim, and the pixel-diamond logo centred inside. The logo is drawn with
whole-pixel cells wherever the size allows, so its pixel edges stay crisp;
the smallest sizes are box-downscaled from the 1024 master.

Tray icons (tray-*.png) are not touched.

Usage: python3 scripts/gen-app-icon.py [--preview out.png]   (needs Pillow)
"""
import io
import struct
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ICONS = Path(__file__).resolve().parent.parent / "apps/desktop/src-tauri/icons"

SPRITE = [
    "......#......",
    ".....###.....",
    "....#####....",
    "...#######...",
    "..#########..",
    ".###########.",
    "####+########",
    ".#######++++.",
    "..#####++++..",
    "...###++++...",
    "....##+++....",
    ".....#++.....",
    "......+......",
]
COLORS = {"#": (222, 227, 238, 255), "+": (128, 129, 133, 255)}
TILE = (36, 37, 40, 255)
CANVAS, TILE_SIZE, RADIUS = 1024, 824, 185
CELL = 28  # logo cell size at 1024 -> 13 * 28 = 364 px glyph
SS = 4  # supersampling for the tile's curved edge


def tile_layer() -> Image.Image:
    """Shadow + rounded tile at 1024, anti-aliased."""
    big = CANVAS * SS
    m = (CANVAS - TILE_SIZE) // 2
    box = [m * SS, m * SS, (m + TILE_SIZE) * SS - 1, (m + TILE_SIZE) * SS - 1]

    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=RADIUS * SS, fill=255)
    mask = mask.resize((CANVAS, CANVAS), Image.LANCZOS)

    # Subtle shadow: offset 10 px down, 12 px blur, 30 % black.
    shadow_a = Image.new("L", (CANVAS, CANVAS), 0)
    shadow_a.paste(mask, (0, 10))
    shadow_a = shadow_a.filter(ImageFilter.GaussianBlur(12)).point(lambda v: v * 30 // 100)
    out = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    out.putalpha(shadow_a)

    # Faint top-to-bottom gradient around TILE so the tile reads on a dark Dock.
    grad = Image.new("RGBA", (1, CANVAS))
    for y in range(CANVAS):
        t = (y - m) / TILE_SIZE
        grad.putpixel((0, y), tuple(round(v + 9 - 16 * t) for v in TILE[:3]) + (255,))
    tile = grad.resize((CANVAS, CANVAS))
    tile.putalpha(mask)
    out = Image.alpha_composite(out, tile)

    # Hairline inner rim (white, 10 %), 3 px at 1024.
    rim = Image.new("L", (big, big), 0)
    ImageDraw.Draw(rim).rounded_rectangle(box, radius=RADIUS * SS, outline=255, width=3 * SS)
    rim = rim.resize((CANVAS, CANVAS), Image.LANCZOS).point(lambda v: v * 10 // 100)
    white = Image.new("RGBA", (CANVAS, CANVAS), (255, 255, 255, 0))
    white.putalpha(rim)
    return Image.alpha_composite(out, white)


def draw_glyph(img: Image.Image, cell: int) -> None:
    n = len(SPRITE)
    off = (img.width - n * cell) // 2
    d = ImageDraw.Draw(img)
    for r, row in enumerate(SPRITE):
        for c, ch in enumerate(row):
            if ch in COLORS:
                x, y = off + c * cell, off + r * cell
                d.rectangle([x, y, x + cell - 1, y + cell - 1], fill=COLORS[ch])


MASTER_TILE = tile_layer()
MASTER = MASTER_TILE.copy()
draw_glyph(MASTER, CELL)


def render(size: int) -> Image.Image:
    if size == CANVAS:
        return MASTER.copy()
    ideal = CELL * size / CANVAS
    cell = max(1, round(ideal))
    if abs(cell - ideal) / ideal > 0.2:  # too small for whole-pixel cells
        return MASTER.resize((size, size), Image.BOX)
    img = MASTER_TILE.resize((size, size), Image.LANCZOS)
    draw_glyph(img, cell)
    return img


def png_bytes(img: Image.Image) -> bytes:
    buf = io.BytesIO()
    img.save(buf, "PNG", optimize=True)
    return buf.getvalue()


def write_icns(path: Path) -> None:
    # PNG-payload element types (macOS 10.7+).
    types = [
        ("icp4", 16), ("icp5", 32), ("ic11", 32), ("icp6", 64), ("ic12", 64),
        ("ic07", 128), ("ic13", 256), ("ic08", 256), ("ic14", 512),
        ("ic09", 512), ("ic10", 1024),
    ]
    cache: dict[int, bytes] = {}
    body = b""
    for t, s in types:
        data = cache.setdefault(s, png_bytes(render(s)))
        body += t.encode() + struct.pack(">I", len(data) + 8) + data
    path.write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)


def main() -> None:
    for name, s in [("32x32.png", 32), ("64x64.png", 64), ("128x128.png", 128),
                    ("128x128@2x.png", 256), ("icon.png", 512)]:
        (ICONS / name).write_bytes(png_bytes(render(s)))
    write_icns(ICONS / "icon.icns")
    ico_sizes = [16, 24, 32, 48, 64, 256]
    imgs = [render(s) for s in ico_sizes]
    imgs[-1].save(ICONS / "icon.ico", format="ICO", sizes=[(s, s) for s in ico_sizes],
                  append_images=imgs[:-1])
    if "--preview" in sys.argv:
        out = Path(sys.argv[sys.argv.index("--preview") + 1])
        preview(out)


def preview(out: Path) -> None:
    """Master icon on light and dark backgrounds, plus the small sizes at 1:1."""
    w, h = 2 * 560 + 40, 760
    pv = Image.new("RGBA", (w, h), (255, 255, 255, 255))
    ImageDraw.Draw(pv).rectangle([w // 2, 0, w, h], fill=(30, 30, 32, 255))
    big = MASTER.resize((512, 512), Image.LANCZOS)
    pv.alpha_composite(big, (24, 24))
    pv.alpha_composite(big, (w // 2 + 24, 24))
    x = 24
    for s in (256, 128, 64, 32, 16):
        if s == 256:
            continue
        img = render(s)
        for base in (0, w // 2):
            pv.alpha_composite(img, (base + x, 560 + (128 - s) // 2 + 30))
        x += s + 30
    pv.save(out)


if __name__ == "__main__":
    main()
