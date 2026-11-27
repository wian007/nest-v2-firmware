# NAND B-slot recovery via USB DFU — WORKS (2026-11-01)

After flashing the B slot and setting bootcmd B-first, the modern kernel did not
come up (panel dark) and a normal boot looped on the hung B. This recovers the
device to stock A over the OMAP3 USB peripheral-boot (DFU) path. PROVEN on the
hardware 2026-11-01: device recovered, stock A booted.

## How it works
Stock u-boot's BUILT-IN default environment is A-first (confirmed in the stock
u-boot binary: `bootcmd=run nandboot0 || run nandboot1 ...`). So erasing the
saved env makes stock u-boot fall back to that default and boot slot A. The
recovery is a dedicated SPL that erases env0+env1 and warm-resets. Erase needs
no ECC, so the stock-vs-mainline BCH4 difference is irrelevant.

## Build + push
```
bash tools/uboot-recovery/build.sh                 # -> out/u-boot-spl.bin (RAW SPL)
python3 tools/omap-usbboot/omap-usbboot.py boot tools/uboot-recovery/out/u-boot-spl.bin
# then power-cycle the Nest HOLDING the DFU gesture (seat-on-base + hold ring)
```
After the transfer the SPL erases the env and resets; stock A boots (stock UI on
the panel, WiFi returns). To re-arm for a fresh ROM window (stale ROM after a
failed attempt is unusable), fully power the unit OFF first.

## The pieces (each was needed)
- `files/0001-...sdram-timings.patch`: get_board_mem_timings = J49 SDRAM from the
  stock x-loader, so the SPL could run (DDR) — though the recovery runs before DDR.
- `files/0002-...env-erase-recovery.patch`: in `board_init_f`, BEFORE
  `omap3_mem_init()` (so a DDR issue can't block it), gpmc_init() + NAND RESET
  (0xFF) + erase blocks 26..31 (env0+env1, abs 0x340000+0xC0000, hard-bounded)
  + PRM warm reset (0x48307250 |= RST_GS). Built with `KCFLAGS=-DNEST_SPL_ENV_ERASE`.
  The NAND RESET was essential: without it the bare erase sequence was ignored.
- Push the RAW `spl/u-boot-spl.bin` (code/reset-vector at offset 0). The ROM copies
  it to 0x40200000 and jumps to offset 0 — a CH- or GP-header-wrapped MLO does NOT
  work (offset 0 isn't executable; the ROM never jumps).
- `tools/omap-usbboot/omap-usbboot.py` fixes (all required on a modern xHCI host):
  read the ASIC ID FIRST (the ROM sends it unsolicited; writing the opcode first
  desyncs it), then 0xF0030002, then the 4-byte size, then the image; pace every
  OUT transfer with a ~1 ms delay and 512-byte chunks (the ROM STALLs if flooded).

## Note
This recovery SPL is for the stuck B slot only. A full J49 U-Boot (items 16-18)
is still future work; the SDRAM port here is the foundation for it.
