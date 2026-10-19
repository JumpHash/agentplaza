#!/usr/bin/env python3
"""Embeds a subset of IBM Plex Mono (400 and 600) in a TUI recording SVG.

usage: python3 scripts/readme/tui-font.py <file.svg> [more.svg ...] [--fonts DIR]

Every character the SVG's <text> elements draw is collected, both faces are subset to those
glyphs (woff2 when brotli is installed, else woff), and the rules are written into the
`/*APZ-FONT*/` placeholder at the start of the SVG's <style>. Running it again replaces the
rules it wrote before, so a re-recorded SVG can simply be passed through again. The faces are
OFL (see the LICENSE.md next to them).
"""

import base64
import io
import os
import re
import sys
import xml.etree.ElementTree as ET

from fontTools import subset
from fontTools.ttLib import TTFont

# The agentplaza.io site checked out beside this repository; --fonts or $APZ_FONTS name another folder.
DEFAULT_FONTS = os.environ.get("APZ_FONTS") or os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "../../../agentplaza-landing/assets/fonts"
)
FACES = [(400, "plex-mono-400-latin.woff2"), (600, "plex-mono-600-latin.woff2")]
MARK = "APZ-FONT"
PLACEHOLDER = re.compile(r"/\*APZ-FONT\*/(?:.*?/\*/APZ-FONT\*/)?", re.S)


def drawn_chars(svg_text):
    """The characters the SVG's text elements draw (zero-width spacers and spaces excluded)."""
    root = ET.fromstring(svg_text)
    chars = set()
    for el in root.iter():
        if el.tag.rsplit("}", 1)[-1] == "text":
            chars.update("".join(el.itertext()))
    chars.discard("​")
    chars.discard(" ")
    return sorted(chars)


def subset_face(path, codepoints, flavor):
    options = subset.Options()
    options.flavor = flavor
    options.hinting = False
    options.glyph_names = False
    options.name_IDs = ["*"]  # keep the copyright and licence records
    options.name_languages = ["*"]
    options.notdef_outline = True
    font = TTFont(path)
    sub = subset.Subsetter(options)
    sub.populate(unicodes=codepoints)
    sub.subset(font)
    font.flavor = flavor
    buf = io.BytesIO()
    font.save(buf)
    return buf.getvalue()


def font_rules(fonts_dir, codepoints):
    flavor = "woff2"
    try:
        import brotli  # noqa: F401
    except ImportError:
        flavor = "woff"
    rules = []
    for weight, name in FACES:
        data = subset_face(os.path.join(fonts_dir, name), codepoints, flavor)
        b64 = base64.b64encode(data).decode("ascii")
        rules.append(
            '@font-face{font-family:"APZ Mono";font-style:normal;font-weight:%d;'
            'src:url(data:font/%s;base64,%s) format("%s")}'
            % (weight, flavor, b64, flavor)
        )
    return "".join(rules)


def embed(svg_path, fonts_dir):
    with open(svg_path, encoding="utf-8") as f:
        text = f.read()
    if "/*APZ-FONT*/" not in text:
        sys.exit(f"{svg_path}: no /*APZ-FONT*/ placeholder in the style")
    codepoints = [ord(c) for c in drawn_chars(text)]
    rules = font_rules(fonts_dir, codepoints)
    text = PLACEHOLDER.sub(
        lambda _m: "/*APZ-FONT*/" + rules + "/*/APZ-FONT*/", text, count=1
    )
    with open(svg_path, "w", encoding="utf-8") as f:
        f.write(text)
    print(
        f"{svg_path}: {len(codepoints)} glyphs per face, {len(text)} bytes",
        file=sys.stderr,
    )


def main(argv):
    fonts_dir = DEFAULT_FONTS
    paths = []
    args = iter(argv)
    for arg in args:
        if arg == "--fonts":
            fonts_dir = next(args, None) or sys.exit("--fonts takes a directory")
        else:
            paths.append(arg)
    if not paths:
        sys.exit(__doc__.strip().splitlines()[2])
    for path in paths:
        embed(path, fonts_dir)


if __name__ == "__main__":
    main(sys.argv[1:])
