#!/usr/bin/env python3
"""Which clocks does clk_disable_unused() gate on this board?

A gate clock is a victim when it is hardware-ON (its enable bit set in the CM
registers sampled by initramfs-init's `STAGE2 CMregs:` lines, in a run booted
WITH clk_ignore_unused so nothing was gated yet) while the clock framework has
enable_cnt == 0 for it (the `NEST-FL: CLK name=enable/prepare@rate` lines).

  unused-victims.py <kernel-src>/arch/arm/boot/dts/ti/omap  decodeN-ramoops.txt

The gate -> (register, bit, inverted) map is parsed from omap3xxx-clocks.dtsi
and omap36xx-clocks.dtsi (ti,*gate*-clock / ti,*interface-clock nodes).
"""
import glob
import re
import sys

CM = 0x48004000
PRM = 0x48306000


def parse_gates(dtsdir):
    """Walk the dtsi files with a node stack: 6.6 groups clocks as children of
    `clock@off { reg = <0x..>; }` nodes, the children carry only ti,bit-shift."""
    gates = {}
    for f in glob.glob(f"{dtsdir}/omap3*-clocks.dtsi"):
        stack = []  # [label, reg, compat, bitshift, inverted]
        for line in open(f):
            m = re.match(r'\s*(?:(\w+):\s*)?([\w@,-]+)\s*\{', line)
            if m:
                stack.append([m.group(1) or m.group(2), None, '', None, False])
                continue
            if not stack:
                continue
            cur = stack[-1]
            if (m := re.search(r'compatible\s*=\s*"([^"]+)"', line)):
                cur[2] = m.group(1)
            elif (m := re.search(r'\breg\s*=\s*<\s*(0x[0-9a-fA-F]+)', line)):
                cur[1] = int(m.group(1), 16)
            elif (m := re.search(r'ti,bit-shift\s*=\s*<\s*(\d+)', line)):
                cur[3] = int(m.group(1))
            elif 'set-bit-to-disable' in line:
                cur[4] = True
            if '}' in line:
                node = stack.pop()
                label, reg, comp, bit, inv = node
                if comp and ('gate' in comp or 'interface' in comp):
                    if reg is None:  # inherit the enclosing clock@off node's register
                        for parent in reversed(stack):
                            if parent[1] is not None:
                                reg = parent[1]
                                break
                    if reg is not None:
                        gates[label] = (reg, bit or 0, inv, comp, 'prm' in f.lower() or f.endswith('prm.dtsi'))
    return gates


def main():
    dtsdir, dump = sys.argv[1], sys.argv[2]
    gates = parse_gates(dtsdir)
    regs, cnt = {}, {}
    for line in open(dump, errors='replace'):
        if 'CMregs:' in line:
            for k, v in re.findall(r'(0x[0-9a-fA-F]+)=0x([0-9A-Fa-f]+)', line):
                regs[int(k, 16)] = int(v, 16)
            m = re.search(r'PRM_CLKOUT_CTRL=0x([0-9A-Fa-f]+)', line)
            if m:
                regs[0x48307d70] = int(m.group(1), 16)
        m = re.search(r'NEST-FL: CLK (\S+)=(\d+)/(\d+)@', line)
        if m:
            cnt[m.group(1)] = int(m.group(2))
    print(f"gates in DT: {len(gates)}, sampled regs: {len(regs)}, clocks with counts: {len(cnt)}")
    print("-- hardware ON and enable_cnt==0  ->  clk_disable_unused() victims:")
    unknown = []
    for name, (off, bit, inv, comp, isprm) in sorted(gates.items()):
        addr = (PRM + off if (isprm or 'clkout1' in name) else CM + off)
        cname = name.replace('_gate_', '_')  # composite gate component -> the registered composite clock
        if addr not in regs:
            unknown.append(name)
            continue
        on = bool(regs[addr] >> bit & 1) ^ inv
        if on and cnt.get(cname, cnt.get(name, 1)) == 0:
            print(f"   {name:26} {comp:34} reg 0x{addr:08x} bit {bit:2}  enable_cnt=0  HW ON")
    print(f"-- {len(unknown)} gates in registers not sampled: {' '.join(unknown)}")


if __name__ == '__main__':
    main()
