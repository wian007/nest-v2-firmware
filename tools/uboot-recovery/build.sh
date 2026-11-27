#!/usr/bin/env bash
# build.sh -- build the Nest J49 NAND B-slot recovery MLO (first-light 2026-11-01).
# A dedicated OMAP3 SPL that, when USB-DFU-booted into SRAM, erases the u-boot env
# partitions (env0+env1, abs 0x340000..0x400000) and warm-resets, so stock u-boot
# reverts to its A-first default env and boots stock A. See RECOVERY.md.
#
# Base: U-Boot 2024.07 omap3_beagle (OMAP3630 GP), + three patches:
#   0001 get_board_mem_timings = J49 SDRAM (from the stock x-loader)
#   0002 spl_board_init env-ERASE recovery (guarded by -DNEST_SPL_ENV_ERASE)
#   0003 spl_board_init env-WRITE recovery (guarded by -DNEST_SPL_ENV_WRITE):
#        writes the factory env back into NAND env0 in STOCK BCH4 ECC.
# Mode select:  NEST_MODE=erase (default) | write
#   erase  -> KCFLAGS=-DNEST_SPL_ENV_ERASE  (runs BEFORE omap3_mem_init)
#   write  -> KCFLAGS=-DNEST_SPL_ENV_WRITE  (runs AFTER  omap3_mem_init; needs DRAM)
# Output: $OUT/MLO  (push with tools/omap-usbboot/omap-usbboot.py boot MLO)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/out}"; mkdir -p "$OUT"
MODE="${NEST_MODE:-erase}"
case "$MODE" in
  erase) NEST_MACRO="-DNEST_SPL_ENV_ERASE" ;;
  write) NEST_MACRO="-DNEST_SPL_ENV_WRITE" ;;
  *) echo "NEST_MODE must be 'erase' or 'write' (got '$MODE')" >&2; exit 1 ;;
esac
echo "=== build mode: $MODE  (KCFLAGS=$NEST_MACRO) ==="
SRC="$OUT/u-boot-2024.07"
if [ ! -d "$SRC" ]; then
  curl -sSL -o "$OUT/u-boot-2024.07.tar.bz2" https://ftp.denx.de/pub/u-boot/u-boot-2024.07.tar.bz2
  tar -C "$OUT" -xjf "$OUT/u-boot-2024.07.tar.bz2"
fi
for p in "$HERE"/../../meta-j49/recipes-bsp/u-boot/files/000*.patch; do
  (cd "$SRC" && patch -p1 -N -r - < "$p" >/dev/null) || true
done
OSSL_DEV=$(nix build --no-link --print-out-paths nixpkgs#openssl.dev)
OSSL_LIB=$(nix build --no-link --print-out-paths nixpkgs#openssl.out)
nix shell \
  nixpkgs#pkgsCross.armv7l-hf-multiplatform.buildPackages.gcc \
  nixpkgs#pkgsCross.armv7l-hf-multiplatform.buildPackages.binutils \
  nixpkgs#gnumake nixpkgs#bison nixpkgs#flex nixpkgs#bc nixpkgs#openssl \
  nixpkgs#python3 nixpkgs#which nixpkgs#coreutils nixpkgs#gawk nixpkgs#findutils \
  --command bash -eu -c "
    cd '$SRC'
    export CROSS_COMPILE=armv7l-unknown-linux-gnueabihf-
    export HOSTCFLAGS='-I$OSSL_DEV/include' HOSTLDFLAGS='-L$OSSL_LIB/lib'
    export KCFLAGS='$NEST_MACRO'
    make omap3_beagle_defconfig >/dev/null
    make -j\$(nproc) >/dev/null
    cp MLO '$OUT/MLO'
    cp spl/u-boot-spl.bin '$OUT/u-boot-spl.bin'   # <- push THIS over DFU (raw SPL, code at offset 0)
  "
echo "=== recovery image (push the raw SPL, not MLO) ==="; ls -la "$OUT/u-boot-spl.bin"
echo "push:  python3 tools/omap-usbboot/omap-usbboot.py boot $OUT/u-boot-spl.bin"
