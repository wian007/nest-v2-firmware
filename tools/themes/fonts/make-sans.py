#!/usr/bin/env python3
"""Make a smooth font for j49-screen (<name>.sfont) from a TrueType or OpenType font.

    make-sans.py <font.ttf> <out.sfont> [--variation Medium]

Needs Pillow with FreeType. The screen draws text at fixed sizes: capitals 7, 14, 21, 35, 42
and 49 pixels high (the "scales" 1, 2, 3, 5, 6 and 7 of the built-in 5x7 font). This renders
the characters at each of those sizes, with 16 levels of coverage per pixel: ASCII and Latin-1
up to 21 pixels, capitals, digits and signs above that.

File format (little endian):
    "J49SFONT1\\n"
    u8  number of sizes
    per size:   u8 height of a capital in pixels, u16 number of glyphs
    per glyph:  u32 code point, u8 width, u8 height, i8 x offset, i8 y offset (top of the
                bitmap below the top of a capital; negative = above), u8 advance,
                then width*height pixels, 4 bits each, high half of the byte first
"""
import struct, sys
from PIL import ImageFont

CAPS = [7, 14, 21, 35, 42, 49]
CHARS = [chr(c) for c in list(range(0x20, 0x7f)) + list(range(0xa0, 0x100))] + ["€", "–", "’"]

args = sys.argv[1:]
variation = None
if "--variation" in args:
    i = args.index("--variation"); variation = args[i + 1]; del args[i:i + 2]
if len(args) != 2:
    sys.exit(__doc__)
src, dst = args

def load(size):
    f = ImageFont.truetype(src, size)
    if variation:
        f.set_variation_by_name(variation)
    return f

out = bytearray(b"J49SFONT1\n") + bytes([len(CAPS)])
for cap in CAPS:
    # the font size whose capital H is `cap` pixels high (the largest that is not higher)
    best = None
    for size in range(cap, cap * 3):
        l, t, r, b = load(size).getbbox("H")
        if b - t <= cap:
            best = size
        else:
            break
    font = load(best)
    cap_top = font.getbbox("H")[1]
    glyphs = []
    # the large sizes are for numbers and a few short words in capitals: no lower case and
    # no accented letters there, which keeps the file small
    for ch in (CHARS if cap <= 21 else [c for c in CHARS if c < "`" or c == "°"]):
        adv = round(font.getlength(ch))
        box = font.getbbox(ch)
        mask = font.getmask(ch, mode="L")
        w, h = mask.size
        if w == 0 or h == 0:
            glyphs.append((ord(ch), 0, 0, 0, 0, adv, b""))
            continue
        px = bytes(mask)
        nib = [min(15, (v + 8) // 17) for v in px]
        if len(nib) % 2:
            nib.append(0)
        data = bytes((nib[i] << 4) | nib[i + 1] for i in range(0, len(nib), 2))
        xoff, yoff = box[0], box[1] - cap_top
        if not (w < 256 and h < 256 and -128 <= xoff < 128 and -128 <= yoff < 128 and adv < 256):
            continue
        glyphs.append((ord(ch), w, h, xoff, yoff, adv, data))
    out += struct.pack("<BH", cap, len(glyphs))
    for cp, w, h, xo, yo, adv, data in glyphs:
        out += struct.pack("<IBBbbB", cp, w, h, xo, yo, adv) + data
open(dst, "wb").write(out)
print("%s: %d bytes, %d characters, sizes %s" % (dst, len(out), len(CHARS), CAPS))
