#!/usr/bin/env bash
# build.sh -- build the stage-2/3 kernel artifacts for the DFU installer
# from a first-light kernel workdir (tools/kernel-firstlight/build.sh) and
# the rootfs jffs2 image:
#
#   out/uImage-j49-a        PRODUCTION kernel: zImage + appended DTB, NO
#                          initramfs, uImage load/entry 0x80008000 (the
#                          2026-11-04 bootm lesson). stage3.sh writes this
#                          into boot0.
#   out/uImage-j49-b      the same production kernel rooted on root1
#                          (mtdblock8), for the B slot.
#   out/zImage-installer   INSTALLER kernel: same build WITH an embedded
#                          initramfs carrying the root0 jffs2 (gz), the
#                          mtdflash helper, j49-screen and the stage-2
#                          init. stage2.sh RAM-boots this via bootz.
#
# Both are the same kernel sources/config as linux-j49_6.6.bb (the
# workdir was built by kernel-firstlight/build.sh from the meta-j49
# recipe files); only CONFIG_INITRAMFS_SOURCE differs. The two kernel
# builds are incremental: only usr/initramfs_data and the link rerun.
#
# Usage: build.sh <kernel-firstlight-workdir> <j49-image.jffs2> [outdir]
set -euo pipefail
WORK="${1:?usage: build.sh <kernel-firstlight-workdir> <jffs2> [outdir]}"
JFFS2="${2:?usage: build.sh <kernel-firstlight-workdir> <jffs2> [outdir]}"
HERE="$(cd "$(dirname "$0")" && pwd)"
NEST_YOCTO="$HERE/../../.."
# Slot and commit go into the kernel's version string, so `uname -v` on the device shows which
# variant runs and from which commit it was built: "#j49b-24ed75b SMP ...". A "+" marks a build
# from a tree with uncommitted changes (the artifacts under installer/out do not count).
REV=$(git -C "$NEST_YOCTO" rev-parse --short HEAD 2>/dev/null || echo unknown)
[ -z "$(git -C "$NEST_YOCTO" status --porcelain --untracked-files=no -- ':(top)' ':(top,exclude)tools/dfu-install/installer/out' 2>/dev/null)" ] || REV="$REV+"
OUT="${3:-$HERE/out}"; mkdir -p "$OUT"
KOUT="$WORK/out"
KSRC=$(ls -d "$WORK"/linux-* 2>/dev/null | head -1 || true)
DTB="$KOUT/arch/arm/boot/dts/ti/omap/omap3-nest-j49.dtb"
CROSS=armv7l-unknown-linux-gnueabihf-

# two supported workdir layouts:
#  A) kernel-firstlight/build.sh output: $WORK/linux-6.6.x + $WORK/out
#  B) Yocto work-shared: $WORK/kernel-source + $WORK/kernel-build-artifacts
#     (patched sources + the exact .config of linux-j49_6.6.bb; we build
#     out-of-tree into $WORK/dfu-build so the Yocto artifacts stay intact)
if [ -f "$KOUT/arch/arm/boot/zImage" ] && [ -d "$KSRC" ]; then
    :
elif [ -d "$WORK/kernel-source" ] && [ -f "$WORK/kernel-build-artifacts/.config" ]; then
    KSRC="$WORK/kernel-source"
    KOUT="$WORK/dfu-build"
    mkdir -p "$KOUT"
    [ -f "$KOUT/.config" ] || cp "$WORK/kernel-build-artifacts/.config" "$KOUT/.config"
    DTB="$KOUT/arch/arm/boot/dts/ti/omap/omap3-nest-j49.dtb"
else
    echo "no kernel source/build in $WORK -- run tools/kernel-firstlight/build.sh $WORK, or pass the Yocto work-shared/nest-j49 dir" >&2
    exit 1
fi
[ -f "$JFFS2" ] || { echo "missing jffs2 $JFFS2" >&2; exit 1; }

# ---------------------------------------------------------------- initramfs
BB=$(nix build --no-link --print-out-paths \
      nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.busybox)/bin/busybox
ROOT="$WORK/installer-root"; rm -rf "$ROOT"
mkdir -p "$ROOT"/{bin,dev,proc,sys,run,mnt,root}
cp "$BB" "$ROOT/bin/busybox"; chmod +x "$ROOT/bin/busybox"
ln -sf busybox "$ROOT/bin/sh"
for a in mount umount gunzip cat head printf sleep reboot sync; do
    ln -sf busybox "$ROOT/bin/$a"
done

# root0 image length for exact progress -- templated into the init. The
# jffs2 is NOT embedded: ~22 MB is nearly incompressible and would blow the
# 64 MB RAM budget at kernel decompression time. stage 2 downloads it into
# DRAM at 0x82000000 (second DFU alt) and the init streams it from /dev/mem.
IMGLEN=$(stat -L -c %s "$JFFS2")
sed "s/@ROOT0_IMG_LEN@/$IMGLEN/" "$HERE/init" > "$ROOT/init"
chmod +x "$ROOT/init"
printf '%s' "$IMGLEN" > "$WORK/rootfs.jffs2.len"
echo "root0 image: $IMGLEN bytes (DFU RAM alt at 0x82000000)"

# device-side binaries (Rust, static musl armv7 -- project convention; the
# flake dev shell supplies the target std and the static linker)
( cd "$NEST_YOCTO" && nix develop --command bash -eu -c '
    cd tools/j49-screen
    cargo build --release --quiet
    cp target/armv7-unknown-linux-musleabihf/release/j49-screen "'"$ROOT"'/bin/j49-screen"
    rustc --target armv7-unknown-linux-musleabihf -O \
        -C target-feature=+crt-static \
        -C linker="$CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER" \
        -o "'"$ROOT"'/bin/nest-mtdflash" "'"$HERE"'/mtdflash.rs"
    CROSSSTRIP="${CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER%cc}strip"
    "$CROSSSTRIP" "'"$ROOT"'/bin/j49-screen" "'"$ROOT"'/bin/nest-mtdflash"
' )

nix shell nixpkgs#cpio nixpkgs#gzip nixpkgs#findutils --command bash -c "
  cd '$ROOT' && find . -print0 | cpio --null -ov --format=newc 2>/dev/null \
    | gzip -9 > '$WORK/installer-initramfs.cpio.gz'
"

# installer DTB: production DTS with root0/boot0 writable + the DFU staging reservation
# (the kernel build below provides scripts/dtc/dtc; the cpp step needs gcc on PATH)
INSTDTB="$WORK/omap3-nest-j49-installer.dtb"
# ------------------------------------------------- two incremental builds
OSSL_DEV=$(nix build --no-link --print-out-paths nixpkgs#openssl.dev)
OSSL_LIB=$(nix build --no-link --print-out-paths nixpkgs#openssl.out)
nix shell \
  nixpkgs#pkgsCross.armv7l-hf-multiplatform.buildPackages.gcc \
  nixpkgs#pkgsCross.armv7l-hf-multiplatform.buildPackages.binutils \
  nixpkgs#gnumake nixpkgs#bison nixpkgs#flex nixpkgs#bc nixpkgs#openssl nixpkgs#perl \
  nixpkgs#which nixpkgs#coreutils nixpkgs#gawk nixpkgs#findutils nixpkgs#diffutils \
  nixpkgs#ubootTools \
  --command bash -eu -c '
    export HOSTCFLAGS="-I'"$OSSL_DEV"'/include"
    export HOSTLDFLAGS="-L'"$OSSL_LIB"'/lib"
    MK="make -C '"$KSRC"' O='"$KOUT"' ARCH=arm CROSS_COMPILE='"$CROSS"' -j$(nproc)"
    CONF='"$KOUT"'/.config

    # (a) production kernel for the A slot (boot0 + root0): initramfs OFF, built-in cmdline
    #     roots on root0. The built-in CONFIG_CMDLINE is appended AFTER U-Boot bootargs and the
    #     last root= wins, so root=/dev/mtdblock6 has to live here (the Yocto recipe roots on
    #     root1 = mtdblock8, the B slot). DTB: root0 writable (jffs2 rw), boot0 stays read-only.
    printf "CONFIG_INITRAMFS_SOURCE=\"\"\nCONFIG_CMDLINE=\"root=/dev/mtdblock6 rootfstype=jffs2 rootwait mem=64M panic=10 mtdoops.mtddev=oops console=ttyS0,115200n8 console=tty0\"\n" > '"$WORK"'/no-initramfs.cfg
    "'"$KSRC"'/scripts/kconfig/merge_config.sh" -m -O '"$KOUT"' "$CONF" '"$WORK"'/no-initramfs.cfg >/dev/null
    $MK olddefconfig
    $MK KBUILD_BUILD_VERSION=j49a-'"$REV"' zImage ti/omap/omap3-nest-j49.dtb
    CPP='"$CROSS"'cpp bash '"$HERE"'/make-installer-dtb.sh '"$KSRC"' '"$KOUT"' '"$WORK"'/omap3-nest-j49-a.dtb a-slot
    cat '"$KOUT"'/arch/arm/boot/zImage '"$WORK"'/omap3-nest-j49-a.dtb > '"$WORK"'/zImage-dtb
    mkimage -A arm -O linux -T kernel -C none -a 0x80008000 -e 0x80008000 \
      -n "j49-6.6-a-'"$REV"'" -d '"$WORK"'/zImage-dtb '"$OUT"'/uImage-j49-a >/dev/null

    # (b) installer kernel: initramfs ON (and the installer DTB: writable root0/boot0 + staging)
    CPP='"$CROSS"'cpp bash '"$HERE"'/make-installer-dtb.sh '"$KSRC"' '"$KOUT"' '"$WORK"'/omap3-nest-j49-installer.dtb
    printf "CONFIG_INITRAMFS_SOURCE=\"%s\"\nCONFIG_INITRAMFS_COMPRESSION_NONE=y\n" \
      '"$WORK"'/installer-initramfs.cpio.gz > '"$WORK"'/installer-initramfs.cfg
    "'"$KSRC"'/scripts/kconfig/merge_config.sh" -m -O '"$KOUT"' "$CONF" '"$WORK"'/installer-initramfs.cfg >/dev/null
    $MK olddefconfig
    $MK KBUILD_BUILD_VERSION=j49inst-'"$REV"' zImage
    cat '"$KOUT"'/arch/arm/boot/zImage '"$WORK"'/omap3-nest-j49-installer.dtb > '"$OUT"'/zImage-installer

    # (c) production kernel for the B slot (boot0 + root1): same as (a) but rooted on root1
    #     (mtdblock8), with the same DTB as (a): root0 AND root1 writable, boot0 read-only. With
    #     the unmodified production DTB root0 was read-only under this kernel, so a system on
    #     root1 could not write a new image to the other slot (found 2026-11-26,
    #     update-from-linux.md).
    printf "CONFIG_INITRAMFS_SOURCE=\"\"\nCONFIG_CMDLINE=\"root=/dev/mtdblock8 rootfstype=jffs2 rootwait mem=64M panic=10 mtdoops.mtddev=oops console=ttyS0,115200n8 console=tty0\"\n" > '"$WORK"'/b-slot.cfg
    "'"$KSRC"'/scripts/kconfig/merge_config.sh" -m -O '"$KOUT"' "$CONF" '"$WORK"'/b-slot.cfg >/dev/null
    $MK olddefconfig
    $MK KBUILD_BUILD_VERSION=j49b-'"$REV"' zImage ti/omap/omap3-nest-j49.dtb
    cat '"$KOUT"'/arch/arm/boot/zImage '"$WORK"'/omap3-nest-j49-a.dtb > '"$WORK"'/zImage-dtb-b
    mkimage -A arm -O linux -T kernel -C none -a 0x80008000 -e 0x80008000 \
      -n "j49-6.6-b-'"$REV"'" -d '"$WORK"'/zImage-dtb-b '"$OUT"'/uImage-j49-b >/dev/null
  '

S_PROD=$(stat -c %s "$OUT/uImage-j49-a" 2>/dev/null || echo 0)
S_INS=$(stat -c %s "$OUT/zImage-installer")
echo "=== installer artifacts ==="
ls -la "$OUT/uImage-j49-a" "$OUT/uImage-j49-b" "$OUT/zImage-installer"
if [ "$S_INS" -le "$S_PROD" ]; then
    echo "WARNING: installer kernel is not larger than the production kernel --" >&2
    echo "         initramfs embedding may have failed" >&2
    exit 1
fi
