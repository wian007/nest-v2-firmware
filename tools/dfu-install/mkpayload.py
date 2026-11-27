#!/usr/bin/env python3
"""mkpayload.py -- build the flashstub payload pushed at 0x82000000.

Usage:
    mkpayload.py --out payload.bin --range first,nblk [--range ...] image.bin

The header is the struct nfd_hdr from flashstub/stub.h (little-endian):
magic, image_len, n_ranges, 0, then {first_block, n_blocks} per range,
then the raw image bytes. Ranges are written BY THE STUB IN THIS ORDER --
put the fallback slot first and the primary slot last (for a u-boot
install: --range 14,12 (spl1) --range 2,12 (spl0)).

The stub hard-refuses anything but spl0/spl1; this tool checks the same
list up front so a typo dies on the host, not on the device.
"""
import argparse
import struct
import sys

ALLOWED = {
    "ipl": (0, 2),
    "spl0": (2, 12),
    "spl1": (14, 12),
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("image")
    ap.add_argument("--out", required=True)
    ap.add_argument("--range", action="append", default=[],
                    help="first_block,n_blocks (absolute 128 KiB blocks)")
    args = ap.parse_args()

    img = open(args.image, "rb").read()
    if not img:
        sys.exit("empty image")
    ranges = []
    for r in args.range:
        first, nblk = (int(x, 0) for x in r.split(","))
        if (first, nblk) not in ALLOWED.values():
            sys.exit(f"refused: range {first},{nblk} is not in the "
                     f"allow-list {sorted(ALLOWED.values())} (ipl is never writable)")
        if len(img) > nblk * 0x20000:
            sys.exit(f"image {len(img)} bytes does not fit {nblk} blocks")
        ranges.append((first, nblk))
    if not ranges:
        sys.exit("no --range given")

    hdr = struct.pack("<IIII", 0x4E464431, len(img), len(ranges), 0)
    for first, nblk in ranges:
        hdr += struct.pack("<II", first, nblk)
    open(args.out, "wb").write(hdr + img)
    print(f"{args.out}: {len(img)} image bytes, ranges: "
          + " -> ".join(f"blk {f}..{f + n - 1}" for f, n in ranges))


if __name__ == "__main__":
    main()
