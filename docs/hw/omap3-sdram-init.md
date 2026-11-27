# J49 OMAP3630 low-level init (extracted from the stock x-loader)

Source: the device's own x-loader, `mtd1_ipl.bin` in the NAND backup
(GP image: 8-byte header `size=0x6c00`, `load=0x40200800`; the x-loader proper
is the 27648 bytes after the header). Disassembled 2026-11-01 (ARM, base
0x40200800) during the NAND B-slot recovery work. These are the authoritative,
known-good values for this exact board — use them for any U-Boot SPL / bare-metal
bring-up instead of guessing from a BeagleBoard variant.

## SDRAM — SDRC @ 0x6D000000 (single CS0 bank, 64 MB mobile DDR)

Init sequence (from the routine at 0x40203bd8):
1. `SDRC_SYSCONFIG (0x10) = 0x2` (soft reset), poll `SDRC_STATUS (0x14)`, then `= 0x0`
2. `SDRC_SHARING (0x44) = 0x00003700`
3. `SDRC_MCFG_0 (0x80) = 0x03582089`
4. `SDRC_ACTIM_CTRLA_0 (0x9C) = 0x629DB4C6`
5. `SDRC_ACTIM_CTRLB_0 (0xA0) = 0x00012114`
6. `SDRC_RFR_CTRL_0 (0xA4) = 0x0004DC01`
7. `SDRC_POWER (0x70) = 0x00000081`
8. manual command sequence via `SDRC_MANUAL_0 (0xA8)`: 0x2 (NOP), delay, 0x1
   (precharge-all), 0x2 (autorefresh), 0x2 (autorefresh)
9. `SDRC_MR_0 (0x84) = 0x00000032` (mode register: CAS latency 3, burst)
10. `SDRC_DLLA_CTRL (0x60) = 0x00000008`

Only CS0 is programmed (MCFG_1/ACTIM_1/RFR_1/MR_1 left default) -> one 64 MB bank
at 0x80000000, matching `mem=62M`/64 MB seen at runtime.

## GPMC @ 0x6E000000 (NAND on CS0)

Init routine at 0x40203d30: `GPMC_SYSCONFIG (0x10) = 0x10`, clear
`GPMC_IRQENABLE (0x1C)`, `GPMC_TIMEOUT_CONTROL (0x40)`, `GPMC_CONFIG (0x78)`;
then per-CS config registers. The CS0 NAND timings match the DTS `gpmc,*-ns`
values already in `omap3-nest-j49.dts`. ECC = BCH4, syndrome bytes at OOB 36..63
(see flash-layout.md).

## Reset

PRM reset control referenced at 0x48307250/0x48307270 (PRM GR module). A warm
reset for the recovery SPL: `PRM_RSTCTRL` RST_GS bit, or arm WDT2 with a short
reload and let it fire.

## Recovery use

A DFU-pushed SPL that only needs to `nand erase` the env (0x340000, len 0xC0000
= env0+env1) and reset does NOT need SDRAM — erase runs from SRAM over GPMC. If
instead it loads a full U-Boot into DRAM, program the SDRC with the values above
first. After erasing the env, stock U-Boot in NAND falls back to its built-in
default environment (A-first) and boots stock A.
