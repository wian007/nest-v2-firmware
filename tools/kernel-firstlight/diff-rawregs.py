#!/usr/bin/env python3
"""Diff the raw CM/PRM/I2C1 register copies that initramfs-init drops into the
post-mortem DRAM window (+0x90000 CM 0x48004000 8 KiB, +0x92000 PRM 0x48306000
8 KiB, +0x94000 I2C1 0x48070000 4 KiB) between two post-mortem dumps, or between
a dump and the stock CM+PRM file from realjump3.sh (CM 8 KiB then PRM 8 KiB).

  diff-rawregs.py postmortem-A.bin postmortem-B.bin
  diff-rawregs.py --stock stock-cm-prm-HHMMSS.bin postmortem-B.bin

Prints every 32-bit word that differs, with the OMAP3 register name when known.
"""
import sys

CM, PRM, I2C = 0x48004000, 0x48306000, 0x48070000
DOMS = {0x000: "IVA2", 0x900: "MPU", 0xa00: "CORE", 0xb00: "SGX", 0xc00: "WKUP", 0xd00: "CKGEN",
        0xe00: "DSS", 0xf00: "CAM", 0x1000: "PER", 0x1100: "EMU", 0x1300: "NEON", 0x1400: "USBHOST"}
CM_OFF = {0x00: "FCLKEN", 0x04: "CLKEN_PLL/FCLKEN2", 0x08: "FCLKEN3", 0x10: "ICLKEN", 0x14: "ICLKEN2",
          0x18: "ICLKEN3", 0x20: "IDLEST", 0x24: "IDLEST2/IDLEST_PLL", 0x28: "IDLEST3", 0x30: "AUTOIDLE",
          0x34: "AUTOIDLE2", 0x38: "AUTOIDLE3", 0x40: "CLKSEL", 0x44: "CLKSEL2", 0x48: "CLKSTCTRL",
          0x4c: "CLKSTST", 0x50: "CLKSEL3", 0x70: "CLKOUT_CTRL/CLKSEL4"}
PRM_NAMES = {0x1200 + 0x20: "PRM_VC_SMPS_SA", 0x1200 + 0x50: "PRM_VOLTCTRL", 0x1200 + 0x58: "PRM_RSTST",
             0x1200 + 0x70: "PRM_CLKSRC_CTRL", 0x1200 + 0x90: "PRM_OBS", 0x1200 + 0xb0: "PRM_VP1_CONFIG",
             0x1200 + 0xd0: "PRM_VP2_CONFIG", 0x1d00 + 0x70: "PRM_CLKOUT_CTRL", 0x1d00 + 0x40: "PRM_CLKSEL",
             0x1d00 + 0x0c: "PRM_IRQSTATUS?"}
I2C_NAMES = {0x10: "SYSC", 0x24: "IRQSTATUS_RAW", 0x28: "IRQSTATUS", 0x2c: "IRQENABLE_SET", 0x34: "WE",
             0x90: "SYSS", 0x94: "BUF", 0x98: "CNT", 0xa4: "CON", 0xa8: "OA", 0xac: "SA", 0xb0: "PSC",
             0xb4: "SCLL", 0xb8: "SCLH", 0xbc: "SYSTEST", 0xc0: "BUFSTAT"}


def name(base, off):
    if base == CM:
        dom = DOMS.get(off & ~0xff, f"@{off & ~0xff:x}")
        return f"CM_{CM_OFF.get(off & 0xff, f'+{off & 0xff:02x}')}_{dom}"
    if base == PRM:
        return PRM_NAMES.get(off, f"PRM+0x{off:04x}")
    return f"I2C1_{I2C_NAMES.get(off, f'+{off:02x}')}"


def regions(path, stock=False):
    d = open(path, 'rb').read()
    if stock:
        return {CM: d[0:0x2000], PRM: d[0x2000:0x4000]}
    return {CM: d[0x90000:0x92000], PRM: d[0x92000:0x94000], I2C: d[0x94000:0x95000]}


def main():
    args = sys.argv[1:]
    stock = False
    if args and args[0] == '--stock':
        stock = True
        args = args[1:]
    a = regions(args[0], stock)
    b = regions(args[1])
    for base in (CM, PRM, I2C):
        if base not in a or base not in b:
            continue
        ra, rb = a[base], b[base]
        if not any(ra) and not any(rb):
            print(f"-- 0x{base:08x}: both copies empty (dd failed?)")
            continue
        n = 0
        for off in range(0, min(len(ra), len(rb)), 4):
            va = int.from_bytes(ra[off:off + 4], 'little')
            vb = int.from_bytes(rb[off:off + 4], 'little')
            if va != vb:
                lost = va & ~vb
                gained = vb & ~va
                print(f"0x{base + off:08x} {name(base, off):28} A=0x{va:08x} B=0x{vb:08x}"
                      f"  A-only bits {[i for i in range(32) if lost >> i & 1]}"
                      f"  B-only bits {[i for i in range(32) if gained >> i & 1]}")
                n += 1
        print(f"-- 0x{base:08x}: {n} differing words")


if __name__ == '__main__':
    main()
