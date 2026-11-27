#!/usr/bin/env python3
"""Compare OMAP3630 clock-enable state: stock (CM+PRM raw dump from realjump3.sh,
CM 0x48004000 8 KiB followed by PRM 0x48306000 8 KiB) vs the modern kernel's
`STAGE2 CMregs:` line (addr=0xVALUE pairs from the post-mortem log).

  decode-clocks.py stock-cm-prm-HHMMSS.bin [decodeN-ramoops.txt]

Prints every register in both, marks differences and names the bits that stock
has ON and 6.6 has OFF -- those are the clk_disable_unused() victims.
"""
import re
import sys

CM, PRM = 0x48004000, 0x48306000
REGS = {  # addr: (name, {bit: label})
    0x48004a00: ("CM_FCLKEN1_CORE", {0: "MMC1", 1: "MMC2", 2: "MCSPI1", 3: "MCSPI2", 4: "MCSPI3", 5: "MCSPI4",
                                     6: "UART1", 7: "UART2", 8: "GPT10", 9: "GPT11", 10: "MCBSP1", 11: "MCBSP5",
                                     13: "HDQ", 15: "I2C1", 16: "I2C2", 17: "I2C3", 18: "MMC3", 20: "SSI", 24: "MSPRO"}),
    0x48004a08: ("CM_FCLKEN3_CORE", {0: "TS", 1: "CPEFUSE", 2: "USBTLL"}),
    0x48004a10: ("CM_ICLKEN1_CORE", {0: "MMC1", 1: "MMC2", 2: "MCSPI1", 3: "MCSPI2", 4: "MCSPI3", 5: "MCSPI4",
                                     6: "UART1", 7: "UART2", 8: "GPT10", 9: "GPT11", 10: "MCBSP1", 11: "MCBSP5",
                                     12: "MAILBOXES", 13: "HDQ", 15: "I2C1", 16: "I2C2", 17: "I2C3", 18: "MMC3",
                                     19: "ICR", 20: "SSI", 22: "AES2", 23: "SHA12", 24: "MSPRO", 25: "DES2",
                                     27: "AES1", 28: "RNG", 29: "SHA11", 30: "DES1", 31: "PKA"}),
    0x48004a18: ("CM_ICLKEN3_CORE", {0: "TS", 1: "CPEFUSE", 2: "USBTLL"}),
    0x48004c00: ("CM_FCLKEN_WKUP", {0: "GPT1", 3: "GPIO1", 5: "WDT2", 6: "SR1", 7: "SR2", 9: "USIM"}),
    0x48004c10: ("CM_ICLKEN_WKUP", {0: "GPT1", 1: "32KSYNC", 2: "GPT12", 3: "GPIO1", 4: "WDT1", 5: "WDT2", 9: "USIM"}),
    0x48004d00: ("CM_CLKEN_PLL", {0: "DPLL3_EN0", 1: "DPLL3_EN1", 2: "DPLL3_EN2", 16: "DPLL4_EN0", 17: "DPLL4_EN1", 18: "DPLL4_EN2"}),
    0x48004d04: ("CM_CLKEN2_PLL", {0: "DPLL5_EN0", 1: "DPLL5_EN1", 2: "DPLL5_EN2"}),
    0x48004d20: ("CM_IDLEST_CKGEN", {0: "CORE_LOCKED", 1: "PER_LOCKED", 2: "96M_ON", 3: "48M_ON", 4: "12M_ON",
                                     5: "54M_ON", 6: "DSS1_ON", 7: "CAM_ON", 8: "FUNC96M_ON", 9: "SYSCLKOUT2_ON", 10: "SYS_CLKOUT1? ", 11: "EMU_CORE_ON"}),
    0x48004d30: ("CM_AUTOIDLE_PLL", {0: "DPLL3_AUTO0", 1: "DPLL3_AUTO1", 2: "DPLL3_AUTO2", 3: "DPLL4_AUTO0", 4: "DPLL4_AUTO1", 5: "DPLL4_AUTO2"}),
    0x48004d40: ("CM_CLKSEL1_PLL", {}),
    0x48004d70: ("CM_CLKOUT_CTRL", {7: "CLKOUT2_EN(sys_clkout2)", 0: "SRC0", 1: "SRC1", 3: "DIV0", 4: "DIV1", 5: "DIV2"}),
    0x48004e00: ("CM_FCLKEN_DSS", {0: "DSS1", 1: "DSS2", 2: "TV"}),
    0x48004e04: ("(CM_DSS+4, unused)", {}),
    0x48004e10: ("CM_ICLKEN_DSS", {0: "DSS"}),
    0x48004f00: ("CM_FCLKEN_CAM", {0: "CAM", 1: "CSI2"}),
    0x48004f10: ("CM_ICLKEN_CAM", {0: "CAM"}),
    0x48005000: ("CM_FCLKEN_PER", {0: "MCBSP2", 1: "MCBSP3", 2: "MCBSP4", 3: "GPT2", 4: "GPT3", 5: "GPT4", 6: "GPT5",
                                   7: "GPT6", 8: "GPT7", 9: "GPT8", 10: "GPT9", 11: "UART3", 12: "WDT3", 13: "GPIO2",
                                   14: "GPIO3", 15: "GPIO4", 16: "GPIO5", 17: "GPIO6", 18: "UART4"}),
    0x48005010: ("CM_ICLKEN_PER", {0: "MCBSP2", 1: "MCBSP3", 2: "MCBSP4", 3: "GPT2", 4: "GPT3", 5: "GPT4", 6: "GPT5",
                                   7: "GPT6", 8: "GPT7", 9: "GPT8", 10: "GPT9", 11: "UART3", 12: "WDT3", 13: "GPIO2",
                                   14: "GPIO3", 15: "GPIO4", 16: "GPIO5", 17: "GPIO6", 18: "UART4"}),
    0x48005400: ("CM_FCLKEN_USBHOST", {0: "HSUSB_120M", 1: "HSUSB_48M"}),
    0x48005410: ("CM_ICLKEN_USBHOST", {0: "HSUSB"}),
    0x48307270: ("PRM_CLKSRC_CTRL", {0: "SYSCLKSEL0", 1: "SYSCLKSEL1", 3: "AUTOEXTCLKMODE0", 4: "AUTOEXTCLKMODE1",
                                     6: "SYSCLKDIV0", 7: "SYSCLKDIV1", 8: "DPLL4_CLKINP_DIV"}),
    0x48307258: ("PRM_CLKSETUP", {}),
    0x48307d70: ("PRM_CLKOUT_CTRL", {7: "CLKOUT_EN(sys_clkout1)"}),
}
ALIASES = {"PRM_CLKOUT_CTRL": 0x48307d70, "PRM_CLKSRC_CTRL": 0x48307270, "PRM_CLKSETUP": 0x48307258}


def stock_regs(path):
    d = open(path, 'rb').read()
    out = {}
    for a in REGS:
        if CM <= a < CM + 0x2000:
            o = a - CM
        elif PRM <= a < PRM + 0x2000:
            o = 0x2000 + a - PRM
        else:
            continue
        if o + 4 <= len(d):
            out[a] = int.from_bytes(d[o:o + 4], 'little')
    return out


def modern_regs(path):
    out = {}
    for line in open(path, errors='replace'):
        if 'CMregs:' not in line:
            continue
        for k, v in re.findall(r'(\w+)=0x([0-9A-Fa-f]+)', line):
            a = int(k, 16) if k.startswith('0x') else ALIASES.get(k)
            if a:
                out[a] = int(v, 16)
    return out


def bits(a, v):
    return ' '.join(lbl for b, lbl in REGS[a][1].items() if v >> b & 1)


def main():
    st = stock_regs(sys.argv[1])
    md = modern_regs(sys.argv[2]) if len(sys.argv) > 2 else {}
    print(f"{'register':20} {'stock':>10} {'6.6':>10}  notes")
    for a, (name, _) in sorted(REGS.items()):
        s, m = st.get(a), md.get(a)
        ss = f"0x{s:08x}" if s is not None else "-"
        ms = f"0x{m:08x}" if m is not None else "-"
        note = ""
        if s is not None and m is not None and s != m:
            lost = bits(a, s & ~m)
            gained = bits(a, m & ~s)
            note = "DIFF" + (f"  stock-ON/6.6-OFF: {lost}" if lost else "") + (f"  6.6-ON only: {gained}" if gained else "")
        elif s is not None:
            note = bits(a, s)
        print(f"{name:20} {ss:>10} {ms:>10}  {note}")


if __name__ == '__main__':
    main()
