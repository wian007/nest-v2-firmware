#!/usr/bin/env python3
"""Decode the Nest J49 post-mortem DRAM window (0x83e00000..0x83f00000, 1 MiB)
dumped from stock via memdump after the watchdog warm reset.

  ramoops console zone @ +0x00000 (256 KiB): persistent_ram header
      u32 sig ('DBGC' = 0x43474244), u32 start, u32 size, then text
  DEBUG_LL DRAM ring  @ +0x80000 (64 KiB): u32 idx (dup at +4), copy A @ +0x100,
      copy B @ +0x8100, each 0x7f00 bytes (arch/arm/include/debug/ramring.S)

DRAM loses ~1% of set bits (1->0 only) across the OMAP3 warm reset, so the ring's
two copies are OR-merged and the ramoops signature is matched with a popcount
tolerance. Verdicts:
  ring empty            -> kexec never ran the zImage (or the ring macro never fired)
  ring has banner only  -> decompressor ran, kernel died before earlyprintk
  ring has kernel text  -> read it: it ends where the kernel hung
  ramoops valid         -> kernel reached device-init; full console log follows
"""
import sys

RAMOOPS_OFF, RAMOOPS_SZ = 0x00000, 0x40000
RING_OFF, RING_DATA, RING_COPY_B, RING_LEN = 0x80000, 0x100, 0x8100, 0x7f00
SIG = 0x43474244


def u32(b, o):
    return int.from_bytes(b[o:o + 4], 'little')


def near(a, b, maxbits=4):
    return bin(a ^ b).count('1') <= maxbits


def printable(bs):
    return ''.join(chr(c) if 32 <= c < 127 or c in (9, 10, 13) else ('�' if c else '')
                   for c in bs)


BREADCRUMBS = [('A', 'zImage entered (kexec jumped)'),
               ('B', 'past cache_on'),
               ('C', 'appended DTB / ATAG compat done'),
               ('D', 'calling decompress_kernel')]


def decode_breadcrumbs(r):
    """head.S ringmark bytes at +0x10.. with a duplicate at +0x20.. (OR-merged)."""
    got = []
    for i, (ch, what) in enumerate(BREADCRUMBS):
        v = r[0x10 + i] | r[0x20 + i]
        got.append((ch, what, v == ord(ch), v))
    print("== decompressor breadcrumbs: " + ' '.join(
        f"{ch}{'=' if ok else '!'}{v:02x}" for ch, _, ok, v in got))
    for ch, what, ok, _ in got:
        print(f"   {'reached ' if ok else 'MISSING '} {ch}: {what}")
    reached = [ch for ch, _, ok, _ in got if ok]
    if not reached:
        print("   -> not even 'A': kexec never transferred control to the zImage entry")
    elif len(reached) < len(BREADCRUMBS):
        print(f"   -> died between {reached[-1]} and {BREADCRUMBS[len(reached)][0]}")
    return reached


def decode_ring(d):
    r = d[RING_OFF:RING_OFF + 0x10000]
    decode_breadcrumbs(r)
    idx, idx2 = u32(r, 0), u32(r, 4)
    a = r[RING_DATA:RING_DATA + RING_LEN]
    b = r[RING_COPY_B:RING_COPY_B + RING_LEN]
    merged = bytes(x | y for x, y in zip(a, b))
    used = sum(1 for c in merged if c)
    print(f"== DEBUG_LL ring: idx=0x{idx:x} (dup 0x{idx2:x}), non-zero bytes={used}, "
          f"A/B mismatches={sum(1 for x, y in zip(a, b) if x != y)}")
    if used == 0:
        print("   RING EMPTY -> nothing ever executed the DEBUG_LL macro (zImage never entered?)")
        return False
    end = idx if 0 < idx <= RING_LEN and merged[idx:].count(0) == len(merged) - idx else RING_LEN
    text = merged[:end].rstrip(b'\0')
    print(f"-- ring text ({len(text)} bytes) --")
    print(printable(text))
    print("-- end ring --")
    return True


def decode_ramoops(d):
    z = d[RAMOOPS_OFF:RAMOOPS_OFF + RAMOOPS_SZ]
    sig, start, size = u32(z, 0), u32(z, 4), u32(z, 8)
    print(f"== ramoops: sig=0x{sig:08x} start=0x{start:x} size=0x{size:x}")
    if not near(sig, SIG):
        nz = sum(1 for c in z if c)
        print(f"   NO ramoops header (non-zero bytes in zone: {nz}) -> kernel never reached ramoops probe")
        return False
    cap = RAMOOPS_SZ - 12
    data = z[12:12 + cap]
    size = min(size, cap)
    if size and start <= size:
        text = data[start:size] + data[:start]
    else:
        text = data[:size] if size else data.rstrip(b'\0')
    print(f"-- ramoops console ({len(text)} bytes) --")
    print(printable(text))
    print("-- end ramoops --")
    return True


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    d = open(sys.argv[1], 'rb').read()
    if len(d) < RING_OFF + 0x10000:
        sys.exit(f"dump too small ({len(d)} bytes); need the full 1 MiB window")
    ring = decode_ring(d)
    ro = decode_ramoops(d)
    print("== verdict:", "ramoops valid (kernel reached device-init)" if ro else
          "kernel entered but died before ramoops probe -- read the ring" if ring else
          "nothing written -> kexec did not execute the zImage")


if __name__ == '__main__':
    main()
