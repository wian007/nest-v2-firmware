#!/usr/bin/env bash
# build-stock-kernel.sh -- rebuild the nest-dfu-attack STOCK 2.6.37 rescue
# kernel with an auto-install initramfs, producing the session-1 payload:
#
#   payload/nest-install-uImage
#
# The initramfs gains /ub.img (our U-Boot uImage) and /etc/init.d/nestinstall,
# which writes spl0+spl1 through the stock kernel's NAND driver (= stock BCH4
# ECC) and reboots. This is the proven ECC route: no custom bare-metal NAND
# writer involved, and it gives panel feedback (logo + tty1 text).
#
# Build environment: the attack repo's own Dockerfile (ubuntu:22.04 + the
# 2008q3 i686 toolchain via lib32z1). The kernel tree is already built, so
# only the initramfs object + link are rebuilt.
#
# IMPORTANT: the initramfs is extracted and repacked inside the container AS
# ROOT. The base initramfs holds 97 device nodes (dev/console, dev/null,
# dev/tty1, dev/ttyGS0, dev/mtd*, ...). Extracting as a normal user silently
# drops them (cpio: Cannot mknod); the kernel then has no /dev/console, so no
# ttyGS0 shell, nestinstall never runs and stock boots again. Nothing is
# staged on the host, and the node count is verified after packing.
#
# Usage: build-stock-kernel.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
DFU_ATTACK="${DFU_ATTACK:-$HOME/Projects/nest-dfu-attack}"
DEV="$DFU_ATTACK/Dev"
UBIMG="${UBIMG:-$HERE/../uboot/out/u-boot-nest-j49.uimg}"      # written to spl0
UBIMG1="${UBIMG1:-$UBIMG}"                                     # written to spl1
OUT="${OUT:-$HERE/../payload/nest-install-uImage}"

[ -f "$UBIMG" ] || { echo "missing $UBIMG -- build uboot/build.sh first" >&2; exit 1; }
[ -f "$DEV/initramfs_data.cpio" ] || { echo "missing $DEV/initramfs_data.cpio" >&2; exit 1; }

# 1. inputs for the container: our U-Boot image + the nestinstall script
WORK="$(mktemp -d "${TMPDIR:-/tmp}/nest-install-in.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
cp "$UBIMG" "$WORK/ub0.img"
cp "$UBIMG1" "$WORK/ub1.img"
cat > "$WORK/nestinstall" <<'NESTINSTALL'
#!/bin/sh
# nestinstall -- session 1 on the STOCK kernel (RAM rescue over DFU).
say() { echo "[nestinstall] $*" > /dev/tty1 2>/dev/null; echo "[nestinstall] $*"; }
# on failure keep the box (and the ttyGS0 shell) up for 2 min so it can be read
fail() { say "$* FAILED -- reboot in 120 s"; sync; sleep 120; reboot -f; }
say "start: writing U-Boot to spl0/spl1 (stock BCH4)"
/usr/sbin/flash_eraseall /dev/mtd2 || fail "erase spl0"
/usr/sbin/nandwrite -p /dev/mtd2 /ub0.img || fail "write spl0"
say "spl0 written"
/usr/sbin/flash_eraseall /dev/mtd3 || fail "erase spl1"
/usr/sbin/nandwrite -p /dev/mtd3 /ub1.img || fail "write spl1"
say "spl1 written -- sync + reboot"
sync
reboot -f
NESTINSTALL

cat > "$WORK/fixrcs.sh" <<'FIXRCS'
#!/bin/sh
# insert ${INITDIR}/nestinstall before the line that starts "start"
f="$1"
grep -q nestinstall "$f" && exit 0
awk '/INITDIR}\/start$/ && !d { print "${INITDIR}/nestinstall"; d=1 } { print }' "$f" > "$f.new"
grep -q nestinstall "$f.new" || { echo "FATAL: no start line in rcS" >&2; exit 1; }
cat "$f.new" > "$f"; rm -f "$f.new"
FIXRCS
cat > "$WORK/checklibs.sh" <<'CHECKLIBS'
#!/bin/sh
S="$1"
PATH=/src/Dev/toolchain/arm-2008q3/bin:$PATH
for b in usr/sbin/flash_eraseall usr/sbin/nandwrite bin/busybox; do
  for l in $(arm-none-linux-gnueabi-readelf -d "$S/$b" | sed -n 's/.*NEEDED.*\[\(.*\)\]/\1/p'); do
    [ -e "$S/lib/$l" ] || [ -e "$S/usr/lib/$l" ] || { echo "FATAL: $b needs $l" >&2; exit 1; }
  done
done
echo "libs ok"
CHECKLIBS

# 2. stage + pack as root in the container (keeps the device nodes)
BACKUP="${TMPDIR:-/tmp}/initramfs_data_gen.cpio.orig"
cp "$DEV/initramfs_data_gen.cpio" "$BACKUP" 2>/dev/null || true
IMG=$(docker build -q "$DFU_ATTACK")
docker run --rm -v "$DFU_ATTACK:/src" -v "$WORK:/in:ro" "$IMG" bash -euc '
  S=/tmp/stage; mkdir -p $S; cd $S
  cpio -idm < /src/Dev/initramfs_data.cpio 2>/dev/null
  cp -rf /src/Dev/root/. $S/
  cp /in/ub0.img $S/ub0.img
  cp /in/ub1.img $S/ub1.img
  install -m 755 /in/nestinstall $S/etc/init.d/nestinstall
  # libgcc_s is not in the stock initramfs (it lives on the rootfs there);
  # flash_eraseall/nandwrite/ps need it
  cp /src/Dev/toolchain/arm-2008q3/arm-none-linux-gnueabi/libc/lib/libgcc_s.so.1 $S/lib/libgcc_s.so.1
  # run nestinstall BEFORE "start": start blocks rcS forever, so an appended
  # nestinstall was never reached
  sh /in/fixrcs.sh $S/etc/init.d/rcS
  grep -nE "nestinstall|INITDIR}/start" $S/etc/init.d/rcS
  # every NEEDED lib of the flash tools must exist in the image
  sh /in/checklibs.sh $S
  find . -print0 | LC_ALL=C sort -z | cpio -o0 -H newc > /src/Dev/initramfs_data_gen.cpio 2>/dev/null
  n=$(cpio -tv < /src/Dev/initramfs_data_gen.cpio 2>/dev/null | grep -cE "^[cb]")
  echo "device nodes in packed initramfs: $n"
  [ "$n" -ge 90 ] || { echo "FATAL: device nodes lost" >&2; exit 1; }
  for d in console null tty1 ttyGS0 mtd2 mtd3; do
    cpio -tv < /src/Dev/initramfs_data_gen.cpio 2>/dev/null | grep -qE " dev/$d\$" \
      || { echo "FATAL: dev/$d missing" >&2; exit 1; }
  done
  chown "$(stat -c %u:%g /src/Dev)" /src/Dev/initramfs_data_gen.cpio
'

# 3. incremental kernel rebuild in the attack repo's docker environment
docker run --rm -v "$DFU_ATTACK:/src" -w /src/Dev "$IMG" bash -lc '
  export PATH=/src/Dev/toolchain/arm-2008q3/bin:$PATH
  # delete only GENERATED files: initramfs_data.S is tracked source
  rm -f linux/usr/initramfs_data.o linux/usr/initramfs_data.cpio* linux/usr/.initramfs_data.*
  make -C linux ARCH=arm CROSS_COMPILE=arm-none-linux-gnueabi- -j$(nproc) uImage
'

mkdir -p "$HERE/../payload"
cp "$DEV/linux/arch/arm/boot/uImage" "$OUT"

# 4. leave their tree as it was (cpio restored; kernel uImage stays rebuilt)
if [ -f "$BACKUP" ]; then
    cp "$BACKUP" "$DEV/initramfs_data_gen.cpio"
fi

ls -la "$OUT"
echo "session 1 payload ready: $OUT"
