#!/usr/bin/env python3
"""Draws the AgentPlaza app icon (the hive mark: three hexagonal cells in ink around the signal-
orange core, on a paper tile) from the same geometry as the landing page's scripts/gen-assets.py (JumpHash/agentplaza-landing), and
writes every size the platforms want: packaging/icons/agentplaza-<n>.png, agentplaza.ico
(Windows), agentplaza.icns (macOS). Needs Pillow. Run: python3 packaging/scripts/gen-icons.py"""
import math, pathlib
from PIL import Image, ImageDraw

OUT = pathlib.Path(__file__).resolve().parents[1] / "icons"
PAPER, INK, SIGNAL = (233, 236, 229, 255), (18, 22, 19, 255), (232, 89, 12, 255)
S = 4096  # drawn at 4x 1024, then downsampled (antialiasing)

def hexagon(cx, cy, r):
    return [(cx + r * math.cos(math.radians(a)), cy + r * math.sin(math.radians(a))) for a in range(0, 360, 60)]

def icon():
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # The tile: macOS' grid (an 824/1024 rounded square), a 1px-ink rule and the print shadow.
    m, rad = S * 100 // 1024, S * 185 // 1024
    sh = S * 22 // 1024
    d.rounded_rectangle([m + sh, m + sh, S - m + sh, S - m + sh], rad, fill=INK)
    d.rounded_rectangle([m, m, S - m, S - m], rad, fill=PAPER, outline=INK, width=S * 14 // 1024)
    # The hive, in the mark's 32-unit box scaled into the tile.
    unit = (S - 2 * m) / 32 * 0.86
    ox, oy = S / 2 - 16 * unit, S / 2 - 16.9 * unit
    cell_r, gap, cx, cy = 5.6, 1.2, 16.0, 18.75
    dd = math.sqrt(3) * cell_r + gap
    ring = [(cx, cy - dd), (cx - dd * math.cos(math.pi / 6), cy + dd * math.sin(math.pi / 6)),
            (cx + dd * math.cos(math.pi / 6), cy + dd * math.sin(math.pi / 6))]
    to = lambda pts: [(ox + x * unit, oy + y * unit) for x, y in pts]
    for x, y in ring:
        pts = to(hexagon(x, y, cell_r - 0.4))
        d.line(pts + pts[:2], fill=INK, width=round(2.1 * unit), joint="curve")
    d.polygon(to(hexagon(cx, cy, cell_r)), fill=SIGNAL)
    return img.resize((1024, 1024), Image.LANCZOS)

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    big = icon()
    for n in (16, 24, 32, 48, 64, 128, 256, 512, 1024):
        big.resize((n, n), Image.LANCZOS).save(OUT / f"agentplaza-{n}.png")
    big.save(OUT / "agentplaza.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    big.save(OUT / "agentplaza.icns")
    print("icons:", sorted(p.name for p in OUT.iterdir()))

main()
