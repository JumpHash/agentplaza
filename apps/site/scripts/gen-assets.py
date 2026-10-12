#!/usr/bin/env python3
"""Generates the site's static assets from the console's sources, so the brand stays one thing:
- assets/hive.svg: the hive mark (apps/web/src/ui/brand.ts geometry);
- assets/logos.svg: a sprite of the model providers' marks (apps/web/src/ui/brands.gen.ts,
  simple-icons, CC0), one <symbol id="…"> each, with their brand colours in data-hex.
Run from anywhere: python3 apps/site/scripts/gen-assets.py
"""
import json, math, pathlib, re

ROOT = pathlib.Path(__file__).resolve().parents[3]
SITE = ROOT / "apps/site"

def hexagon(cx, cy, r):
    pts = []
    for deg in (0, 60, 120, 180, 240, 300):
        a = math.radians(deg)
        pts.append(f"{cx + r * math.cos(a):.2f} {cy + r * math.sin(a):.2f}")
    return "M" + "L".join(pts) + "Z"

CELL_R, GAP, CX, CY = 5.6, 1.2, 16.0, 18.75
d = math.sqrt(3) * CELL_R + GAP
ring = [(CX, CY - d), (CX - d * math.cos(math.pi / 6), CY + d * math.sin(math.pi / 6)), (CX + d * math.cos(math.pi / 6), CY + d * math.sin(math.pi / 6))]
cells = "".join(f'<path d="{hexagon(x, y, CELL_R - 0.4)}" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linejoin="miter"/>' for x, y in ring)
hive = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">{cells}<path class="core" fill="#e8590c" d="{hexagon(CX, CY, CELL_R)}"/></svg>\n'
(SITE / "assets/hive.svg").write_text(hive)

src = (ROOT / "apps/web/src/ui/brands.gen.ts").read_text()
body = src[src.index("{", src.index("BRAND_ICONS")) : src.rindex("}") + 1]
icons = json.loads(re.sub(r",\s*}", "}", body))
WANT = ["anthropic", "openai", "google", "xai", "deepseek", "moonshot", "mistral", "openrouter", "perplexity",
        "nvidia", "ollama", "lmstudio", "huggingface", "replicate", "elevenlabs", "cloudflare", "github"]
symbols = []
for key in WANT:
    icon = icons.get(key)
    if not icon:
        continue
    symbols.append(f'<symbol id="{key}" viewBox="0 0 24 24" data-hex="#{icon["hex"]}"><title>{icon["title"]}</title><path d="{icon["path"]}"/></symbol>')
(SITE / "assets/logos.svg").write_text('<svg xmlns="http://www.w3.org/2000/svg">' + "".join(symbols) + "</svg>\n")
(SITE / "assets/logos.json").write_text(json.dumps({k: {"title": icons[k]["title"], "hex": "#" + icons[k]["hex"]} for k in WANT if k in icons}, indent=1) + "\n")
print("hive.svg, logos.svg:", len(symbols), "marks")
