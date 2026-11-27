#!/usr/bin/env bash
# push.sh -- push a 3-stage image (x-load, u-boot, kernel) to the Nest J49 over
# OMAP3 ROM USB peripheral boot, using our own build (build.sh) of omap_loader.
# Non-destructive by itself: this is a pure RAM boot via the mask-ROM's unsigned
# USB load path (GP device). Nothing on NAND changes unless the thing you boot
# does it (see README.md).
#
# Usage: push.sh <x-load.bin> <u-boot.bin> <uImage|zImage-kx> [out-dir]
#
# Before running: plug the Nest into USB, then put it in DFU mode (hold the
# display 10-15s until it reboots) -- this opens the ROM's USB boot window.
set -euo pipefail
XLOAD="${1:?usage: push.sh <x-load.bin> <u-boot.bin> <kernel-image> [out-dir]}"
UBOOT="${2:?need u-boot.bin}"
KERNEL="${3:?need kernel image (uImage or appended zImage+dtb)}"
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${4:-$HERE/out}"
LOADER="$OUT/omap_loader"

[ -x "$LOADER" ] || { echo "missing $LOADER -- run build.sh first" >&2; exit 1; }
for f in "$XLOAD" "$UBOOT" "$KERNEL"; do [ -f "$f" ] || { echo "missing $f" >&2; exit 1; }; done

echo "[*] waiting for the OMAP3 ROM (0451:d00e) -- put the Nest in DFU mode now"
echo "    (plug in USB, hold the display 10-15s until it reboots)"
exec "$LOADER" \
  -f "$XLOAD" \
  -f "$UBOOT" -a 0x80100000 \
  -f "$KERNEL" -a 0x80A00000 \
  -v -j 0x80100000
