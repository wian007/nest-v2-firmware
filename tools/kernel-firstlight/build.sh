#!/usr/bin/env bash
# Build a mainline ARM (OMAP3) kernel + nest DTB + a busybox initramfs for a
# hands-free kexec first-light on the Nest J49. Toolchain comes from nixpkgs
# (cross gcc); no zig. Produces: $OUT/arch/arm/boot/zImage,
# .../dts/ti/omap/omap3-nest-j49.dtb, and $WORK/initramfs.cpio.gz.
#
# Usage: build.sh <workdir> [kver]
set -euo pipefail
WORK="${1:?usage: build.sh <workdir> [kver]}"
KVER="${2:-6.6.64}"
HERE="$(cd "$(dirname "$0")" && pwd)"
KFILES="$HERE/../../meta-j49/recipes-kernel/linux/files"   # patches, j49.cfg, DTS live in the Yocto layer
SRC="$WORK/linux-$KVER"
OUT="$WORK/out"
CROSS=armv7l-unknown-linux-gnueabihf-
mkdir -p "$WORK" "$OUT"

# 1. kernel source
if [ ! -d "$SRC" ]; then
  curl -sSL -o "$WORK/linux-$KVER.tar.xz" \
    "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$KVER.tar.xz"
  tar -C "$WORK" -xf "$WORK/linux-$KVER.tar.xz"
fi

# 1b. our kernel patches (single source of truth = the meta-j49 kernel recipe); idempotent
for pt in "$KFILES"/0*.patch; do
  (cd "$SRC" && patch -p1 -N -r - < "$pt" >/dev/null) || true
done

# 2. our DT into the tree + register it
cp "$KFILES/omap3-nest-j49.dts" \
   "$SRC/arch/arm/boot/dts/ti/omap/omap3-nest-j49.dts"
grep -q omap3-nest-j49.dtb "$SRC/arch/arm/boot/dts/ti/omap/Makefile" || \
  sed -i 's#\(\s*\)omap3-beagle\.dtb \\#\1omap3-beagle.dtb \\\n\1omap3-nest-j49.dtb \\#' \
    "$SRC/arch/arm/boot/dts/ti/omap/Makefile"

# 3. build (openssl host headers/libs needed for certs/extract-cert;
#    note HOSTLDFLAGS, not KBUILD_HOSTLDFLAGS — the top Makefile resets the latter)
OSSL_DEV=$(nix build --no-link --print-out-paths nixpkgs#openssl.dev)
OSSL_LIB=$(nix build --no-link --print-out-paths nixpkgs#openssl.out)

nix shell \
  nixpkgs#pkgsCross.armv7l-hf-multiplatform.buildPackages.gcc \
  nixpkgs#pkgsCross.armv7l-hf-multiplatform.buildPackages.binutils \
  nixpkgs#gnumake nixpkgs#bison nixpkgs#flex nixpkgs#bc nixpkgs#openssl nixpkgs#perl \
  nixpkgs#which nixpkgs#coreutils nixpkgs#gawk nixpkgs#findutils nixpkgs#diffutils \
  nixpkgs#gzip nixpkgs#cpio nixpkgs#gmp nixpkgs#libmpc nixpkgs#elfutils \
  --command bash -eu -c '
    SRC="'"$SRC"'"; OUT="'"$OUT"'"
    export HOSTCFLAGS="-I'"$OSSL_DEV"'/include"
    export HOSTLDFLAGS="-L'"$OSSL_LIB"'/lib"
    MK="make -C $SRC O=$OUT ARCH=arm CROSS_COMPILE='"$CROSS"' -j$(nproc)"
    $MK omap2plus_defconfig
    "$SRC/scripts/kconfig/merge_config.sh" -m -O "$OUT" "$OUT/.config" "'"$KFILES"'/j49.cfg"
    $MK olddefconfig
    $MK zImage ti/omap/omap3-nest-j49.dtb
    # cpufreq-dt as a loadable module (j49.cfg CPUFREQ_DT=m): its built-in probe hangs the
    # board before /init arms the watchdog (jumps 60-63); insmod it over SSH instead.
    if grep -q "^CONFIG_CPUFREQ_DT=m" "$OUT/.config"; then
      $MK drivers/cpufreq/cpufreq-dt.ko && cp "$OUT/drivers/cpufreq/cpufreq-dt.ko" "'"$WORK"'/cpufreq-dt.ko"
    fi
  '

# 4. initramfs (static musl busybox + our init; device nodes via DEVTMPFS_MOUNT)
BB=$(nix build --no-link --print-out-paths \
      nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.busybox)/bin/busybox
ROOT="$WORK/initramfs-root"; rm -rf "$ROOT"; mkdir -p "$ROOT"/{bin,dev,proc,sys,root}
cp "$BB" "$ROOT/bin/busybox"; chmod +x "$ROOT/bin/busybox"
ln -sf busybox "$ROOT/bin/sh"
cp "$HERE/initramfs-init" "$ROOT/init"; chmod +x "$ROOT/init"
# i2cprobe: /dev/i2c-N register-read probe used by init for the TWL4030 0x4b hunt
nix shell 'nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.buildPackages.gcc' --command \
  armv7l-unknown-linux-musleabihf-gcc -Os -static -o "$ROOT/bin/i2cprobe" "$HERE/i2cprobe.c"
# arec: tiny raw-ioctl ALSA capture (no alsa-lib) for the USB sound-card mic test
nix shell 'nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.buildPackages.gcc' --command \
  armv7l-unknown-linux-musleabihf-gcc -Os -static -o "$ROOT/bin/arec" "$HERE/arec.c"
nix shell nixpkgs#cpio nixpkgs#gzip nixpkgs#findutils --command bash -c "
  cd '$ROOT' && find . -print0 | cpio --null -ov --format=newc 2>/dev/null | gzip -9 > '$WORK/initramfs.cpio.gz'
"

# 5. appended-DTB blob for kexec from the ATAG-era stock kernel. kexec-tools loads
#    only header-_magic_end bytes of a zImage and DISCARDS an appended DTB, so bump
#    that header word to the full length (the decompressor finds the DTB via its
#    own LC0 _edata, which is untouched). -> $WORK/zImage-dtb-kx is what you kexec.
cat "$OUT/arch/arm/boot/zImage" "$OUT/arch/arm/boot/dts/ti/omap/omap3-nest-j49.dtb" > "$WORK/zImage-dtb"
python3 - "$WORK/zImage-dtb" "$WORK/zImage-dtb-kx" <<'PY'
import struct, sys
d = bytearray(open(sys.argv[1], 'rb').read())
while len(d) % 4: d.append(0)
d[0x2c:0x30] = struct.pack('<I', len(d)); open(sys.argv[2], 'wb').write(d)
PY

echo "=== artifacts ==="
ls -la "$OUT/arch/arm/boot/zImage" \
       "$OUT/arch/arm/boot/dts/ti/omap/omap3-nest-j49.dtb" \
       "$WORK/initramfs.cpio.gz" "$WORK/zImage-dtb-kx"
