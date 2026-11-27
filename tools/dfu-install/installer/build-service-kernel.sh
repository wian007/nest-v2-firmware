#!/usr/bin/env bash
# Build the service payload of the installer in the browser: the 2.6.37 rescue kernel (GPL,
# from the NestDFUAttack / cuckoo-nest sources) with a small initramfs of our own:
#
#   /init            nandio (../nandio/nandio.c), statically linked: it is the only program
#                    that runs, and it talks to the installer on the USB serial gadget
#   /bin/busybox     with its applet links and the libraries it needs, for the SHELL command
#                    of nandio (repairs by hand); taken from the rescue initramfs of
#                    NestDFUAttack (busybox is GPL-2.0, the libraries are glibc, LGPL)
#   /dev/...         the device nodes nandio needs before devtmpfs is mounted
#
# Nothing else of the stock system is in it: no vendor scripts, settings, certificates or
# firmware. Why this kernel and not ours: partitions that the stock boot chain reads must be
# written with the ECC format of this kernel's NAND driver, and a stock device can only be
# read without errors in that format.
#
# Needs Docker and a NestDFUAttack tree (see ../nest-dfu/README.md): DFU_ATTACK=<dir>.
# Output: ../payload/nest-service-uImage (OUT=<file> for another place).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
DFU_ATTACK="${DFU_ATTACK:-$HOME/Projects/nest-dfu-attack}"
DEV="$DFU_ATTACK/Dev"
OUT="${OUT:-$HERE/../payload/nest-service-uImage}"
[ -f "$DEV/initramfs_data.cpio" ] || { echo "missing $DEV/initramfs_data.cpio (DFU_ATTACK=<NestDFUAttack tree>)" >&2; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/nest-service-in.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
cp "$HERE/../nandio/nandio.c" "$HERE/../nandio/font5x7.h" "$WORK/"

# Everything below runs as root in the container: device nodes cannot be made otherwise, and
# a cpio packed without them boots into a system without a console.
cat > "$WORK/stage.sh" <<'STAGE'
set -eu
export PATH=/src/Dev/toolchain/arm-2008q3/bin:$PATH
OLD=/tmp/old; S=/tmp/stage
mkdir -p $OLD $S && cd $OLD && cpio -idm < /src/Dev/initramfs_data.cpio 2>/dev/null
cd $S
mkdir -p bin lib dev proc sys tmp
chmod 1777 tmp

# nandio, static, as /init
cp /in/font5x7.h /tmp/font5x7.h; cp /in/nandio.c /tmp/nandio.c
arm-none-linux-gnueabi-gcc -O2 -Wall -static -o init /tmp/nandio.c -lm
arm-none-linux-gnueabi-strip init
arm-none-linux-gnueabi-readelf -d init 2>/dev/null | grep -q NEEDED && { echo "FATAL: init is not static" >&2; exit 1; }

# busybox and what it needs. A library is a real file plus one or more links to it
# (libc.so.6 -> libc-2.11.1.so): both are copied, the link as a link.
cp -a $OLD/bin/busybox bin/busybox
copy_lib() {
    f="$1"
    [ -e "$S/lib/$f" ] || [ -L "$S/lib/$f" ] && return 0
    [ -e "$OLD/lib/$f" ] || { echo "FATAL: no library $f" >&2; exit 1; }
    cp -a "$OLD/lib/$f" "$S/lib/$f"
    if [ -L "$OLD/lib/$f" ]; then copy_lib "$(readlink "$OLD/lib/$f")"; fi
    for n in $(arm-none-linux-gnueabi-readelf -d "$OLD/lib/$f" 2>/dev/null | sed -n 's/.*NEEDED.*\[\(.*\)\]/\1/p'); do copy_lib "$n"; done
}
for n in $(arm-none-linux-gnueabi-readelf -d bin/busybox | sed -n 's/.*NEEDED.*\[\(.*\)\]/\1/p'); do copy_lib "$n"; done
interp=$(arm-none-linux-gnueabi-readelf -l bin/busybox | sed -n 's/.*program interpreter: \(.*\)\]/\1/p')
copy_lib "$(basename "$interp")"
# the applets a person needs at the rescue shell
for a in sh ash cat ls cp mv rm mkdir mount umount dmesg ps kill sleep echo dd hexdump grep head tail sync reboot df free vi; do
    ln -s busybox bin/$a
done
ln -s bin sbin

# device nodes (numbers as in the stock initramfs; devtmpfs replaces them when it mounts)
mknod -m 600 dev/console c 5 1
mknod -m 666 dev/null c 1 3
mknod -m 600 dev/tty0 c 4 0
mknod -m 600 dev/tty1 c 4 1
mknod -m 640 dev/kmsg c 1 11
mknod -m 640 dev/ttyGS0 c 252 0
i=0; while [ $i -le 15 ]; do mknod -m 644 dev/mtd$i c 90 $((i * 2)); i=$((i + 1)); done

# Pack. The order matters: a directory must come before what is in it, and a link target is
# resolved when the system runs, not here. Sorted names give that order.
find . -print0 | LC_ALL=C sort -z | cpio -o0 -H newc > /src/Dev/initramfs_data_gen.cpio 2>/dev/null
L=$(cpio -tv < /src/Dev/initramfs_data_gen.cpio 2>/dev/null)
echo "$L" | awk '{print $1, $NF, $(NF-1) == "->" ? "" : ""}' | sed 's/ $//' | head -60
n=$(echo "$L" | grep -cE "^[cb]"); echo "device nodes: $n"; [ "$n" -eq 22 ] || { echo "FATAL: device nodes" >&2; exit 1; }
echo "$L" | grep -qE "^-rwx.* init$" || { echo "FATAL: no /init" >&2; exit 1; }
echo "$L" | grep -cE "^l" | sed 's/^/symlinks: /'
# every link must point at something that is in the archive
for l in $(find . -type l); do [ -e "$l" ] || { echo "FATAL: dangling link $l" >&2; exit 1; }; done
first=$(echo "$L" | sed -n 2p | awk '{print $NF}'); echo "first entry after .: $first"
du -sh . | sed 's/^/initramfs: /'
chown "$(stat -c %u:%g /src/Dev)" /src/Dev/initramfs_data_gen.cpio
STAGE

BACKUP="${TMPDIR:-/tmp}/initramfs_data_gen.cpio.orig"
cp "$DEV/initramfs_data_gen.cpio" "$BACKUP" 2>/dev/null || true
IMG=$(docker build -q "$DFU_ATTACK")
docker run --rm -v "$DFU_ATTACK:/src" -v "$WORK:/in:ro" "$IMG" bash /in/stage.sh
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
