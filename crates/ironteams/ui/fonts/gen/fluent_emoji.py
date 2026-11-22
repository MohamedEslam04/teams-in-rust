#!/usr/bin/env python3
"""Builds FluentEmoji3D.ttf, a CBDT/CBLC colour bitmap font, from Microsoft's Fluent Emoji (3D).

Usage:
    python3 -m venv venv && venv/bin/pip install fonttools pillow
    git clone --filter=blob:none --sparse --depth 1 https://github.com/microsoft/fluentui-emoji.git SRC
    git -C SRC sparse-checkout set --no-cone '/LICENSE' '/assets/*/metadata.json' \
        '/assets/*/3D/*' '/assets/*/*/3D/*'
    venv/bin/python fluent_emoji.py SRC/assets ../FluentEmoji3D.ttf [--size 96] [--colors 256] \
        [--tones all|default]

Slint's renderer draws CBDT bitmaps (not COLRv1), hence a bitmap font. Each emoji becomes one
glyph with one PNG strike (image format 17). Single code points go in the cmap; sequences (ZWJ,
keycaps, skin tones) become GSUB `ccmp` ligatures, also in their FE0F-less spelling. Country
flags do not exist in Fluent Emoji, so they are left to the next font in the chain.

The source tree is treated as data only: nothing in it is executed.
"""

import argparse
import io
import json
import os
import re
import sys

from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import newTable
from fontTools.ttLib.tables.BitmapGlyphMetrics import SmallGlyphMetrics
from fontTools.ttLib.tables.C_B_D_T_ import cbdt_bitmap_format_17
from fontTools.ttLib.tables.E_B_L_C_ import (
    BitmapSizeTable,
    SbitLineMetrics,
    Strike,
    eblc_index_sub_table_1,
)
from PIL import Image

UPEM = 2048
# The font's vertical metrics, as a fraction of the em: close to Twemoji's, so a line holding an
# emoji does not grow taller than the same line in the UI font.
ASCENT, DESCENT = 1905, 364

# Skin-tone modifier -> folder name in the Fluent repo.
TONES = {
    0x1F3FB: "Light",
    0x1F3FC: "Medium-Light",
    0x1F3FD: "Medium",
    0x1F3FE: "Medium-Dark",
    0x1F3FF: "Dark",
}
# Code points that only join other emoji; they get an empty glyph.
JOINERS = {0x200D, 0xFE0F, 0x20E3}
KEYCAP_BASES = set(map(ord, "0123456789#*"))


def parse_seq(text):
    return tuple(int(x, 16) for x in text.split())


def collect(assets, tones):
    """Returns {sequence: png_path} for every emoji, with its skin-tone variants."""
    out = {}
    for name in sorted(os.listdir(assets)):
        meta_path = os.path.join(assets, name, "metadata.json")
        if not os.path.isfile(meta_path):
            continue
        with open(meta_path, encoding="utf-8") as f:
            meta = json.load(f)
        base = os.path.join(assets, name)
        seqs = {parse_seq(meta["unicode"]): "Default"}
        if tones == "all":
            for s in meta.get("unicodeSkintones", []):
                seq = parse_seq(s)
                tone = [TONES[c] for c in seq if c in TONES]
                if tone:
                    seqs[seq] = tone[0]
        for seq, folder in seqs.items():
            d = os.path.join(base, "3D") if not os.path.isdir(os.path.join(base, folder)) else os.path.join(base, folder, "3D")
            pngs = sorted(p for p in os.listdir(d) if p.endswith(".png")) if os.path.isdir(d) else []
            if len(pngs) != 1:
                print(f"skip {name}/{folder}: {len(pngs)} PNGs", file=sys.stderr)
                continue
            out[seq] = os.path.join(d, pngs[0])
    return out


def make_png(path, size, colors):
    """Downscales to a square `size` canvas and quantises to an 8-bit palette with alpha."""
    im = Image.open(path).convert("RGBA")
    # Premultiplied resize, so transparent edge pixels do not bleed their colour in.
    pm = im.convert("RGBa")
    scale = min(size / pm.width, size / pm.height)
    w, h = max(1, round(pm.width * scale)), max(1, round(pm.height * scale))
    pm = pm.resize((w, h), Image.LANCZOS).convert("RGBA")
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    canvas.paste(pm, ((size - w) // 2, (size - h) // 2))
    q = canvas.quantize(colors=colors, method=Image.Quantize.FASTOCTREE, dither=Image.Dither.NONE)
    buf = io.BytesIO()
    q.save(buf, "PNG", optimize=True)
    return buf.getvalue()


def build(assets, out, size, colors, tones):
    entries = collect(assets, tones)
    # Glyphs in a stable order: by sequence.
    seqs = sorted(entries)
    gname = {s: "g_" + "_".join(f"{c:04x}" for c in s) for s in seqs}

    singles = {s[0] for s in seqs if len(s) == 1}
    # A text-default emoji ("2764 fe0f") also answers to its bare code point.
    bare = {s[0]: s for s in seqs if len(s) == 2 and s[1] == 0xFE0F}
    cmap_src = {}
    for cp in singles:
        cmap_src[cp] = gname[(cp,)]
    for cp, s in bare.items():
        cmap_src.setdefault(cp, gname[s])

    # Components of sequences that need a glyph of their own to be matched in GSUB.
    components = set()
    for s in seqs:
        if len(s) > 1:
            components.update(s)
    empty_cps = sorted(c for c in components if c not in cmap_src)
    print("components without an own glyph (invisible glyph):", [hex(c) for c in empty_cps])

    # Each joiner gets its own invisible glyph so GSUB can tell the code points apart.
    cname = {cp: f"c_{cp:04x}" for cp in empty_cps}
    order = [".notdef"] + list(cname.values()) + [gname[s] for s in seqs]
    cmap = dict(cmap_src)
    for cp in empty_cps:
        cmap[cp] = cname[cp]

    adv_px = round(size * 1.08)
    adv_units = round(adv_px / size * UPEM)
    fb = FontBuilder(UPEM, isTTF=True)
    fb.setupGlyphOrder(order)
    fb.setupCharacterMap(cmap)
    empty = TTGlyphPen(None).glyph()
    fb.setupGlyf({g: empty for g in order})
    metrics = {g: (adv_units, 0) for g in order}
    for g in [".notdef"] + list(cname.values()):
        metrics[g] = (0, 0)
    fb.setupHorizontalMetrics(metrics)
    fb.setupHorizontalHeader(ascent=ASCENT, descent=-DESCENT)
    fb.setupNameTable(
        {
            "familyName": "Fluent Emoji 3D",
            "styleName": "Regular",
            "uniqueFontIdentifier": "FluentEmoji3D-Regular",
            "fullName": "Fluent Emoji 3D",
            "psName": "FluentEmoji3D-Regular",
            "version": "Version 1.000",
            "copyright": "Copyright (c) Microsoft Corporation. MIT License.",
        }
    )
    fb.setupOS2(
        sTypoAscender=ASCENT,
        sTypoDescender=-DESCENT,
        sTypoLineGap=0,
        usWinAscent=ASCENT,
        usWinDescent=DESCENT,
        achVendID="IRTM",
    )
    fb.setupPost(keepGlyphNames=False)
    font = fb.font

    # GSUB: a ligature per sequence, longest first (feaLib sorts them), also without FE0F.
    rules = {}
    for s in seqs:
        if len(s) > 1:
            rules[s] = gname[s]
            stripped = tuple(c for c in s if c != 0xFE0F)
            if stripped != s and len(stripped) > 1:
                rules.setdefault(stripped, gname[s])
    def glyph_of(cp):
        return cmap[cp]
    fea = ["languagesystem DFLT dflt;", "languagesystem latn dflt;", "feature ccmp {"]
    for s, g in sorted(rules.items()):
        fea.append(f"  sub {' '.join(glyph_of(c) for c in s)} by {g};")
    fea.append("} ccmp;")
    addOpenTypeFeaturesFromString(font, "\n".join(fea))

    # CBDT / CBLC: one strike, image format 17 (small metrics + PNG), index format 1.
    bearing_x = (adv_px - size) // 2
    bearing_y = round(size * 0.85)
    strike_data = {}
    total = 0
    for s in seqs:
        png = make_png(entries[s], size, colors)
        total += len(png)
        g = cbdt_bitmap_format_17(b"", font)
        g.metrics = SmallGlyphMetrics()
        g.metrics.height = size
        g.metrics.width = size
        g.metrics.BearingX = bearing_x
        g.metrics.BearingY = bearing_y
        g.metrics.Advance = adv_px
        g.imageData = png
        strike_data[gname[s]] = g
    print(f"{len(seqs)} glyphs, {total / 1e6:.2f} MB of PNG")

    sub = eblc_index_sub_table_1(b"", font)
    sub.indexFormat = 1
    sub.imageFormat = 17
    sub.names = [gname[s] for s in seqs]
    sub.imageDataOffset = 0
    sub.locations = []
    strike = Strike()
    bst = strike.bitmapSizeTable
    bst.colorRef = 0
    for side in ("hori", "vert"):
        m = SbitLineMetrics()
        m.ascender = round(ASCENT / UPEM * size)
        m.descender = -round(DESCENT / UPEM * size)
        m.widthMax = adv_px
        m.caretSlopeNumerator = 0
        m.caretSlopeDenominator = 0
        m.caretOffset = 0
        m.minOriginSB = 0
        m.minAdvanceSB = 0
        m.maxBeforeBL = 0
        m.minAfterBL = 0
        m.pad1 = 0
        m.pad2 = 0
        setattr(bst, side, m)
    bst.startGlyphIndex = 0
    bst.endGlyphIndex = 0
    bst.ppemX = size
    bst.ppemY = size
    bst.bitDepth = 32
    bst.flags = 1
    strike.indexSubTables = [sub]

    cblc = newTable("CBLC")
    cblc.version = 3.0
    cblc.strikes = [strike]
    cbdt = newTable("CBDT")
    cbdt.version = 3.0
    cbdt.strikeData = [strike_data]
    font["CBLC"] = cblc
    font["CBDT"] = cbdt

    font.save(out)
    print(f"wrote {out}: {os.path.getsize(out) / 1e6:.2f} MB")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("assets", help="fluentui-emoji checkout's assets/ directory")
    ap.add_argument("out", help="output .ttf")
    ap.add_argument("--size", type=int, default=96, help="strike size in px (default 96)")
    ap.add_argument("--colors", type=int, default=256, help="palette size per PNG (default 256)")
    ap.add_argument("--tones", choices=["all", "default"], default="all")
    a = ap.parse_args()
    build(a.assets, a.out, a.size, a.colors, a.tones)


if __name__ == "__main__":
    main()
