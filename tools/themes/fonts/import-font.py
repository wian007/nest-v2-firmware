#!/usr/bin/env python3
"""Turn a bitmap font from somewhere else into a .font file for j49-screen (5 wide, 7 high).

    import-font.py bdf <file.bdf> [--to U+00FF]     an X11 BDF font with a 5x7 cell
    import-font.py rows <font.h>                     C source with glyphs as 7 strings of 5 characters
    import-font.py sfd <file.sfd> [<file.sfd> ...]   FontForge files that builds glyphs from dots on a 100-unit grid

The result goes to standard output; put your own comment with the source and the licence on top.
Glyphs that do not fit in 5x7 are left out (the screen then uses the built-in glyph).
"""
import re, sys

def out(glyphs):
    for ch in sorted(glyphs):
        rows = glyphs[ch]
        if not any("#" in r for r in rows):
            continue
        name = ch if "!" <= ch <= "~" else "U+%04X" % ord(ch)
        print("\nglyph " + name)
        print("\n".join(rows))

def bdf(path, last):
    glyphs, ascent = {}, 6
    lines = open(path, encoding="latin-1").read().split("\n")
    i = 0
    while i < len(lines):
        l = lines[i]
        if l.startswith("FONT_ASCENT"):
            ascent = int(l.split()[1])
        if l.startswith("ENCODING"):
            code = int(l.split()[1])
        if l.startswith("BBX"):
            w, h, xo, yo = map(int, l.split()[1:5])
        if l == "BITMAP":
            rows = [["."] * 5 for _ in range(7)]
            ok = 0x21 <= code <= last
            for r in range(h):
                bits = int(lines[i + 1 + r], 16) << 8 >> (len(lines[i + 1 + r]) * 4)   # left aligned, 8 bits
                y = (ascent - 1) - (yo + h - 1) + r
                for x in range(w):
                    if bits & (0x80 >> x):
                        if 0 <= y < 7 and 0 <= x + xo < 5: rows[y][x + xo] = "#"
                        else: ok = False
            if ok:
                glyphs[chr(code)] = ["".join(r) for r in rows]
            i += h
        i += 1
    return glyphs

def rows(path):
    text = open(path).read()
    glyphs = {}
    for m in re.finditer(r"\{'(\\?.)',\s*\{((?:\s*\"[^\"]{5}\",?){7})\s*\}\}", text):
        ch = m[1][-1] if m[1] != "\\\\" else "\\"
        if m[1] == "\\'": ch = "'"
        glyphs[ch] = [r.replace(" ", ".") for r in re.findall(r'"([^"]{5})"', m[2])]
    return glyphs

def sfd(paths):
    glyphs = {}
    # several files: a glyph in a later file replaces the one of an earlier file
    for block in [b for p in paths for b in open(p).read().split("StartChar: ")[1:]]:
        enc = re.search(r"^Encoding: \d+ (-?\d+)", block, re.M)
        if not enc or not 0x21 <= int(enc[1]) <= 0x7e:
            continue
        grid, ok, n = [["."] * 5 for _ in range(7)], True, 0
        for m in re.finditer(r"^Refer: \d+ -?\d+ \w 1 0 0 1 (-?\d+) (-?\d+)", block, re.M):
            x, y = (int(m[1]) - 50) // 100, 6 - int(m[2]) // 100
            if (int(m[1]) - 50) % 100 or int(m[2]) % 100 or not (0 <= x < 5 and 0 <= y < 7):
                ok = False
            else:
                grid[y][x] = "#"; n += 1
        if ok and n:
            # a narrow glyph of a proportional font goes to the middle of the cell
            used = [x for x in range(5) if any(r[x] == "#" for r in grid)]
            shift = (4 - used[-1]) // 2 if used[0] == 0 else 0
            glyphs[chr(int(enc[1]))] = ["".join((["."] * shift + r)[:5]) for r in grid]
        else:
            glyphs.pop(chr(int(enc[1])), None)
    return glyphs

if len(sys.argv) < 3 or sys.argv[1] not in ("bdf", "rows", "sfd"):
    sys.exit(__doc__)
last = int(sys.argv[sys.argv.index("--to") + 1].replace("U+", ""), 16) if "--to" in sys.argv else 0xff
files = [a for a in sys.argv[2:] if not a.startswith("--") and not a.startswith("U+")]
out({"bdf": lambda p: bdf(p[0], last), "rows": lambda p: rows(p[0]), "sfd": sfd}[sys.argv[1]](files))
