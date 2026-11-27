#!/usr/bin/env bash
# stage1.sh -- session 1 of the DFU installer: replace spl0/spl1 with our
# U-Boot via the ROM USB-peripheral-boot window (one DFU gesture).
#
# Chain (identical in shape to the proven tools/omap-loader/push.sh flow):
#   ROM <- x-load.bin (SRAM, the vendored USB loader)
#   x-load <- flashstub.bin @0x80100000   (where u-boot.bin normally goes)
#   x-load <- payload.bin  @0x82000000    (NFD1 header + u-boot uImage)
#   x-load jumps to 0x80100000 -> stub draws on the panel, writes spl1 then
#   spl0 in stock BCH4 (stock x-loader keeps loading u-boot the same way),
#   verifies, warm-resets. ~10 s later OUR u-boot's DFU gadget appears on
#   USB (g_dnl, vendor:product below) = success signal for stage 2.
#
# Usage: stage1.sh <u-boot-image> [x-load.bin]
#   <u-boot-image>  the mkimage-wrapped U-Boot (see ../uboot/README.md:
#                  -O u-boot -A arm -T firmware -C none -a 0x80100000 -e 0,
#                  exactly like the stock mtd2_spl0.bin layout)
#   [x-load.bin]    default: ~/Projects/nest-dfu-attack/Release/Nest/x-load.bin
#
# Before running: put the Nest in DFU mode (plug USB, hold the ring 10-15 s
# until it reboots -> 0451:d00e appears).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
IMG="${1:?usage: stage1.sh <u-boot-image> [x-load.bin]}"
XLOAD="${2:-$HOME/Projects/nest-dfu-attack/Release/Nest/x-load.bin}"
LOADER="$HERE/../omap-loader/out/omap_loader"
STUB="$HERE/flashstub/out/flashstub.bin"

[ -x "$LOADER" ] || { echo "missing $LOADER -- build tools/omap-loader first" >&2; exit 1; }
[ -f "$STUB" ]   || { echo "missing $STUB -- run flashstub/build.sh first" >&2; exit 1; }
[ -f "$XLOAD" ]  || { echo "missing x-load.bin ($XLOAD)" >&2; exit 1; }
[ -f "$IMG" ]    || { echo "missing image $IMG" >&2; exit 1; }

PAYLOAD="$(mktemp /tmp/opencode/nfd1-payload.XXXXXX.bin)"
trap 'rm -f "$PAYLOAD"' EXIT
python3 "$HERE/mkpayload.py" --out "$PAYLOAD" \
    --range 14,12 --range 2,12 "$IMG"

echo "[*] waiting for the OMAP3 ROM (0451:d00e) -- put the Nest in DFU mode now"
echo "    (plug in USB, hold the ring 10-15 s until it reboots)"
"$LOADER" \
    -f "$XLOAD" \
    -f "$STUB" -a 0x80100000 \
    -f "$PAYLOAD" -a 0x82000000 \
    -v -j 0x80100000

echo "[*] pushed. The panel should show the installer now:"
echo "    WAIT -> ERASE -> WRITE -> VERIFY -> DONE -> reboot."
echo "[*] success = our u-boot DFU gadget on USB within ~15 s:"
echo "    lsusb | grep -i 0451:df10   (then run stage2.sh)"
