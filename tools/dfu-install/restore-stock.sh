#!/usr/bin/env bash
# restore-stock.sh -- put the ORIGINAL boot chain back via the ROM DFU path:
#   1. stock u-boot (mtd2_spl0.bin) -> spl1 then spl0, in stock BCH4, with
#      the same flashstub as stage 1 (payload ranges 14,12 and 2,12)
#   2. factory env -> env0 in stock BCH4, with the existing NEST_MODE=write
#      SPL from tools/uboot-recovery (bit-exact verified 2026-11-04)
#
# Between the two pushes you must re-enter the ROM window (fully power the
# unit OFF first, then seat-on-base + hold the ring 10-15 s): after step 1
# warm-resets into stock and the ROM window is gone.
#
# Usage: restore-stock.sh [mtd2_spl0.bin]
#   default: the newest backup under $J49_BACKUP_DIR/*/mtd2_spl0.bin
#
# NOTE: step 2 wipes our U-Boot env (it rewrites env0 in stock format), so
# after a restore the device is factory-stock again (WiFi regdom/pairing
# vars restored) -- that is the point.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SPL0="${1:-$(ls -1t "${J49_BACKUP_DIR:-.}"/*/mtd2_spl0.bin 2>/dev/null | head -1)}"
[ -n "${SPL0:-}" ] && [ -f "$SPL0" ] || { echo "no mtd2_spl0.bin backup found (pass one explicitly)" >&2; exit 1; }
XLOAD="${XLOAD:-$HERE/nest-dfu/x-load.bin}"
LOADER="$HERE/../omap-loader/out/omap_loader"
STUB="$HERE/flashstub/out/flashstub.bin"
ENVSPL="$HERE/../uboot-recovery/out/u-boot-spl.bin"

[ -x "$LOADER" ] || { echo "missing $LOADER -- build tools/omap-loader first" >&2; exit 1; }
[ -f "$STUB" ]   || { echo "missing $STUB -- run flashstub/build.sh first" >&2; exit 1; }
[ -f "$XLOAD" ]  || { echo "missing x-load.bin ($XLOAD)" >&2; exit 1; }

echo "[1/2] stock u-boot -> spl1+spl0 (stock BCH4, via the flashstub)"
PAYLOAD="$(mktemp /tmp/opencode/nfd1-stock.XXXXXX.bin)"
trap 'rm -f "$PAYLOAD"' EXIT
python3 "$HERE/mkpayload.py" --out "$PAYLOAD" --range 14,12 --range 2,12 "$SPL0"
echo "[*] put the Nest in DFU mode now: power OFF, plug USB, hold the ring 10-15 s"
"$LOADER" -f "$XLOAD" -f "$STUB" -a 0x80100000 -f "$PAYLOAD" -a 0x82000000 -v -j 0x80100000
echo "[*] stock u-boot restored; the device warm-resets and stock boots."

echo
echo "[2/2] factory env -> env0 (stock BCH4)"
if [ ! -f "$ENVSPL" ]; then
    echo "missing $ENVSPL -- build it first:"
    echo "    NEST_MODE=write bash tools/uboot-recovery/build.sh"
    exit 1
fi
echo "[*] power the Nest OFF again, then re-enter DFU (hold the ring 10-15 s)"
python3 "$HERE/../omap-usbboot/omap-usbboot.py" boot "$ENVSPL"
echo "[*] env restored. Stock boots with the full factory environment."
