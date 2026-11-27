#!/usr/bin/env bash
# Build the service payload of the installer in the browser: the stock 2.6.37 rescue kernel
# with an initramfs that starts `nandio` (../nandio/nandio.c) on the USB serial gadget instead
# of a shell. Through it the installer makes the backup, writes our U-Boot into spl0/spl1 and
# puts a backup back. Unlike nest-install-uImage it holds no image of ours: the browser sends
# what must be written.
#
# Needs Docker and a NestDFUAttack tree (see nest-dfu/README.md): DFU_ATTACK=<dir>.
# Output: ../payload/nest-service-uImage (OUT=<file> for another place).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
DFU_ATTACK="${DFU_ATTACK:-$HOME/Projects/nest-dfu-attack}"
DEV="$DFU_ATTACK/Dev"
OUT="${OUT:-$HERE/../payload/nest-service-uImage}"
[ -f "$DEV/initramfs_data.cpio" ] || { echo "missing $DEV/initramfs_data.cpio (DFU_ATTACK=<NestDFUAttack tree>)" >&2; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/nest-service-in.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
cp "$HERE/../nandio/nandio.c" "$WORK/nandio.c"
# Started from rcS before the normal start-up, so no shell or getty takes the serial line.
# nandio comes back only when the line could not be opened: try again, the gadget may be late.
cat > "$WORK/nestservice" <<'NESTSERVICE'
#!/bin/sh
echo "[nestservice] backup / restore service on USB" > /dev/tty1 2>/dev/null
while :; do
    /usr/sbin/nandio /dev/ttyGS0
    sleep 1
done
NESTSERVICE
cat > "$WORK/fixrcs.sh" <<'FIXRCS'
f="$1"
grep -q nestservice "$f" && exit 0
awk '/INITDIR}\/start$/ && !d { print "${INITDIR}/nestservice"; d=1 } { print }' "$f" > "$f.new"
grep -q nestservice "$f.new" || { echo "FATAL: no start line in rcS" >&2; exit 1; }
cat "$f.new" > "$f"; rm -f "$f.new"
FIXRCS
cat > "$WORK/checklibs.sh" <<'CHECKLIBS'
S="$1"
PATH=/src/Dev/toolchain/arm-2008q3/bin:$PATH
for b in usr/sbin/nandio bin/busybox; do
  for l in $(arm-none-linux-gnueabi-readelf -d "$S/$b" | sed -n 's/.*NEEDED.*\[\(.*\)\]/\1/p'); do
    [ -e "$S/lib/$l" ] || [ -e "$S/usr/lib/$l" ] || { echo "FATAL: $b needs $l" >&2; exit 1; }
  done
done
echo "libs ok"
CHECKLIBS

BACKUP="${TMPDIR:-/tmp}/initramfs_data_gen.cpio.orig"
cp "$DEV/initramfs_data_gen.cpio" "$BACKUP" 2>/dev/null || true
IMG=$(docker build -q "$DFU_ATTACK")
docker run --rm -v "$DFU_ATTACK:/src" -v "$WORK:/in:ro" "$IMG" bash -euc '
  export PATH=/src/Dev/toolchain/arm-2008q3/bin:$PATH
  S=/tmp/stage; mkdir -p $S; cd $S
  cpio -idm < /src/Dev/initramfs_data.cpio 2>/dev/null
  cp -rf /src/Dev/root/. $S/
  arm-none-linux-gnueabi-gcc -O2 -Wall -o $S/usr/sbin/nandio /in/nandio.c
  install -m 755 /in/nestservice $S/etc/init.d/nestservice
  cp /src/Dev/toolchain/arm-2008q3/arm-none-linux-gnueabi/libc/lib/libgcc_s.so.1 $S/lib/libgcc_s.so.1
  sh /in/fixrcs.sh $S/etc/init.d/rcS
  grep -nE "nestservice|INITDIR}/start" $S/etc/init.d/rcS
  sh /in/checklibs.sh $S
  find . -print0 | LC_ALL=C sort -z | cpio -o0 -H newc > /src/Dev/initramfs_data_gen.cpio 2>/dev/null
  n=$(cpio -tv < /src/Dev/initramfs_data_gen.cpio 2>/dev/null | grep -cE "^[cb]")
  echo "device nodes in packed initramfs: $n"
  [ "$n" -ge 90 ] || { echo "FATAL: device nodes lost" >&2; exit 1; }
  for d in console null tty1 ttyGS0 mtd2 mtd3 mtd14; do
    cpio -tv < /src/Dev/initramfs_data_gen.cpio 2>/dev/null | grep -qE " dev/$d\$" \
      || { echo "FATAL: dev/$d missing" >&2; exit 1; }
  done
  chown "$(stat -c %u:%g /src/Dev)" /src/Dev/initramfs_data_gen.cpio
'
docker run --rm -v "$DFU_ATTACK:/src" -w /src/Dev "$IMG" bash -lc '
  export PATH=/src/Dev/toolchain/arm-2008q3/bin:$PATH
  rm -f linux/usr/initramfs_data.o linux/usr/initramfs_data.cpio* linux/usr/.initramfs_data.*
  make -C linux ARCH=arm CROSS_COMPILE=arm-none-linux-gnueabi- -j$(nproc) uImage
'
mkdir -p "$(dirname "$OUT")"
cp "$DEV/linux/arch/arm/boot/uImage" "$OUT"
[ -f "$BACKUP" ] && cp "$BACKUP" "$DEV/initramfs_data_gen.cpio"
ls -la "$OUT"
echo "service payload ready: $OUT"
