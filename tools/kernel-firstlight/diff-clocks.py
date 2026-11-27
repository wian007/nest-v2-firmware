#!/usr/bin/env python3
"""Diff the `NEST-FL: CLK name=enable/prepare@rate` lines and the `STAGE2 CMregs:`
line between two post-mortem decodes, e.g. a run WITH clk_ignore_unused (A) and
one WITHOUT (B). Clocks that are hardware-on in A and gated in B are the ones
clk_disable_unused() kills -- the TWL4030/I2C1 culprit is among them.

  diff-clocks.py decodeA-ramoops.txt decodeB-ramoops.txt
"""
import re
import sys


def clocks(path):
    out = {}
    for line in open(path, errors='replace'):
        m = re.search(r'NEST-FL: CLK (\S+)=(\d+)/(\d+)@(\d*)', line)
        if m:
            out[m.group(1)] = (int(m.group(2)), int(m.group(3)), m.group(4))
    return out


def cmregs(path):
    out = {}
    for line in open(path, errors='replace'):
        if 'CMregs:' in line:
            for k, v in re.findall(r'(\w+)=0x([0-9A-Fa-f]+)', line):
                out[k] = int(v, 16)
    return out


def main():
    a, b = sys.argv[1], sys.argv[2]
    ca, cb = clocks(a), clocks(b)
    print(f"clocks: {len(ca)} in A, {len(cb)} in B")
    print("-- enable_cnt==0 clocks (clk_disable_unused candidates) and their rates:")
    for n in sorted(set(ca) | set(cb)):
        ea, eb = ca.get(n), cb.get(n)
        if (ea and ea[0] == 0) or (eb and eb[0] == 0):
            print(f"   {n:28} A={ea} B={eb}" + ("   <- differs" if ea != eb else ""))
    ra, rb = cmregs(a), cmregs(b)
    print("-- CM/PRM register bits ON in A and OFF in B (gated by clk_disable_unused):")
    for k in sorted(set(ra) & set(rb)):
        lost = ra[k] & ~rb[k]
        gained = rb[k] & ~ra[k]
        if lost or gained:
            bits_lost = [i for i in range(32) if lost >> i & 1]
            bits_gained = [i for i in range(32) if gained >> i & 1]
            print(f"   {k}: A=0x{ra[k]:08x} B=0x{rb[k]:08x}  lost bits {bits_lost}  gained bits {bits_gained}")


if __name__ == '__main__':
    main()
