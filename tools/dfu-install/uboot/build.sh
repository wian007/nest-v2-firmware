#!/usr/bin/env bash
# build.sh -- build the DFU installer's U-Boot 2024.07 for the Nest J49
# ("nest_j49"), following the tools/uboot-recovery/build.sh pattern: the
# nixpkgs cross toolchain, the same u-boot-2024.07 tarball, and the
# meta-j49 patches (0001 SDRAM timings keep the SPL buildable -- the SPL
# itself is never used, the STOCK x-loader is our first stage).
#
# What comes out:
#   out/u-boot-nest-j49.uimg   -- push THIS with stage1.sh: a uImage
#       wrapped exactly like the stock mtd2_spl0.bin (mkimage -O u-boot
#       -A arm -T firmware -C none -a 0x80100000 -e 0), so the stock
#       x-loader in ipl loads and runs it unchanged.
#   out/u-boot-spl.bin etc.    -- unused, kept for reference.
#
# Boot behaviour lives in files/nest_j49.env (the whole default env): try
# boot0, else one DFU window (ram installer + nand boot0 alts) and reset.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/out}"; mkdir -p "$OUT"
FILES="$HERE/files"
NEST_YOCTO="$HERE/../../.."

SRC="$OUT/u-boot-2024.07"
if [ ! -d "$SRC" ]; then
    # reuse the extracted tree the recovery build left (it already carries
    # the meta-j49 patches; a fresh tree is copied so the recovery build
    # stays untouched), else fetch the tarball.
    RECOVERY="$NEST_YOCTO/tools/uboot-recovery/out/u-boot-2024.07"
    if [ -d "$RECOVERY" ]; then
        cp -a "$RECOVERY" "$SRC"
    elif [ -f "$NEST_YOCTO/tools/uboot-recovery/out/u-boot-2024.07.tar.bz2" ]; then
        cp "$NEST_YOCTO/tools/uboot-recovery/out/u-boot-2024.07.tar.bz2" "$OUT/"
        tar -C "$OUT" -xjf "$OUT/u-boot-2024.07.tar.bz2"
    else
        curl -sSL -o "$OUT/u-boot-2024.07.tar.bz2" https://ftp.denx.de/pub/u-boot/u-boot-2024.07.tar.bz2
        tar -C "$OUT" -xjf "$OUT/u-boot-2024.07.tar.bz2"
    fi
    # the copied tree may contain the recovery build's artifacts: reset it
    ( cd "$SRC" && make mrproper >/dev/null 2>&1 ) || true
fi

# board overlay every run (idempotent): meta-j49 u-boot patches (-N skips
# applied hunks), our beagle patch, the nest-j49 defconfig/dts/env
for p in "$NEST_YOCTO"/meta-j49/recipes-bsp/u-boot/files/000*.patch "$FILES"/0*.patch; do
    (cd "$SRC" && patch -p1 -N -r - < "$p" >/dev/null) || true
done
cp "$FILES/nest_j49_defconfig" "$SRC/configs/nest_j49_defconfig"
cp "${NEST_DTS:-$FILES/omap3-nest-j49.dts}" "$SRC/arch/arm/dts/omap3-nest-j49.dts"   # NEST_DTS=<file>: probe DTS
cp "${NEST_ENV:-$FILES/nest_j49.env}" "$SRC/nest_j49.env"   # NEST_ENV=<file> builds a probe variant
# build the extra dtb for the beagle target (DEFAULT_DEVICE_TREE points at
# it; the beagle dtb still builds for the SPL config)
grep -q "omap3-nest-j49.dtb" "$SRC/arch/arm/dts/Makefile" || \
    sed -i '/dtb-$(CONFIG_TARGET_OMAP3_BEAGLE) += omap3-beagle.dtb/a dtb-$(CONFIG_TARGET_OMAP3_BEAGLE) += omap3-nest-j49.dtb' \
        "$SRC/arch/arm/dts/Makefile"

# NEST_PROBE_PATCH="<patch> [<patch>...]": apply probe patches for this build
# only and revert them on exit, so normal builds stay clean (see probes/).
if [ -n "${NEST_PROBE_PATCH:-}" ]; then
    for pp in $NEST_PROBE_PATCH; do ( cd "$SRC" && patch -p1 -N < "$pp" >/dev/null ); done
    trap 'for pp in $NEST_PROBE_PATCH; do ( cd "$SRC" && patch -p1 -R < "$pp" >/dev/null ) || true; done' EXIT
fi

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
    make nest_j49_defconfig >/dev/null
    # NEST_EXTRA_CFG=<file>: probe builds append Kconfig lines here
    if [ -n '${NEST_EXTRA_CFG:-}' ]; then
        cat '${NEST_EXTRA_CFG:-}' >> .config; make olddefconfig >/dev/null
    fi
    make -j\$(nproc) >/dev/null   # builds u-boot-dtb.bin + host tools/mkimage
    # wrap EXACTLY like the stock mtd2_spl0.bin header (hexdump-verified):
    # os 0x11 (u-boot), arch 0x02 (arm), type 0x05 (firmware),
    # comp 0x00 (none), load 0x80100000, EP 0x00000000 (yes: zero --
    # the stock x-loader jumps to the load address, not ep)
    tools/mkimage -O u-boot -A arm -T firmware -C none \
        -a 0x80100000 -e 0 -n 'U-Boot 2024.07 nest-j49' \
        -d u-boot-dtb.bin '$OUT/u-boot-nest-j49.uimg' >/dev/null
    cp u-boot-dtb.bin '$OUT/u-boot-dtb.bin'
  "

echo "=== installer U-Boot ==="
ls -la "$OUT/u-boot-nest-j49.uimg"
python3 - "$OUT/u-boot-nest-j49.uimg" <<'EOF'
import struct, sys
d = open(sys.argv[1], "rb").read()
magic, hcrc, t, sz, load, ep, dcrc = struct.unpack(">7I", d[:28])
os_, arch, typ, comp = d[28], d[29], d[30], d[31]
name = d[32:64].rstrip(b"\0").decode()
print(f"magic {magic:#x} (expect 27051956)  load {load:#x} (expect 80100000)  ep {ep:#x} (expect 0)")
print(f"os {os_} (17=u-boot)  arch {arch} (2=arm)  type {typ} (5=firmware)  comp {comp} (0)")
print(f"size {sz}  name {name!r}")
assert magic == 0x27051956 and load == 0x80100000 and ep == 0
assert (os_, arch, typ, comp) == (17, 2, 5, 0)
print("header OK -- byte-for-byte the stock mtd2_spl0.bin shape (11 02 05 00)")
EOF
