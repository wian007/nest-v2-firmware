#!/usr/bin/env bash
# session1-stock.sh -- session 1 via the PROVEN stock-kernel rescue.
#
# Why not the flashstub here: the custom bare-metal stub kept hanging in its
# earliest bring-up (no UART to see where), so this route uses only proven
# code for the first install:
#   ROM <- x-load.bin (nest-dfu-attack) <- u-boot.bin (stock 2012.10) <-
#   nest-install-uImage (STOCK 2.6.37 kernel + our initramfs) @0x80A00000
# The stock u-boot boots the rescue kernel; its /init.d/nestinstall writes
# our U-Boot uImage into spl0+spl1 through the stock kernel's NAND driver
# (= STOCK BCH4 ECC, exactly what stock x-loader reads), shows progress on
# the panel (/dev/tty1) and reboots. Stock x-loader then loads OUR U-Boot,
# which finds boot0 invalid and presents the DFU gadget (0451:df10).
#
# Usage: session1-stock.sh
# Needs: one ROM DFU gesture (hold the ring 10-15 s).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# vendored runtime images (see nest-dfu/README.md); the external attack
# repo is only needed to REBUILD the stock kernel (build-stock-kernel.sh)
NEST_DFU="${NEST_DFU:-$HERE/nest-dfu}"
XLOAD="${XLOAD:-$NEST_DFU/x-load.bin}"
STOCKUBOOT="${STOCKUBOOT:-$NEST_DFU/u-boot.bin}"
KERNEL="${KERNEL:-$HERE/payload/nest-install-uImage}"
LOADER="$HERE/../omap-loader/out/omap_loader"

[ -x "$LOADER" ] || { echo "missing $LOADER -- build tools/omap-loader" >&2; exit 1; }
[ -f "$XLOAD" ] || { echo "missing $XLOAD" >&2; exit 1; }
[ -f "$STOCKUBOOT" ] || { echo "missing $STOCKUBOOT" >&2; exit 1; }
[ -f "$KERNEL" ] || { echo "missing $KERNEL -- build it with installer/build-stock-kernel.sh (see README)" >&2; exit 1; }

echo "[*] waiting for the OMAP3 ROM (0451:d00e) -- put the Nest in DFU mode now"
echo "    (power OFF, plug USB, seat on base, hold the ring 10-15 s)"
"$LOADER" \
    -f "$XLOAD" \
    -f "$STOCKUBOOT" -a 0x80100000 \
    -f "$KERNEL" -a 0x80A00000 \
    -v -j 0x80100000

echo "[*] pushed. The rescue kernel boots (Nest logo + text on the panel),"
echo "    writes spl0/spl1 in stock BCH4 and reboots."
echo "[*] success = our U-Boot DFU gadget within ~20 s: lsusb | grep 0451:df10"
