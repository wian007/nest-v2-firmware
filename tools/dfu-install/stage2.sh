#!/usr/bin/env bash
# stage2.sh -- session 2: push the installer kernel AND the rootfs staging
# image, then boot the installer. NO DFU gesture: the device is already in
# our U-Boot, which fell through bootcmd into `dfu 0` (gadget 0451:df10).
#
# Two downloads in the same window:
#   installer -> RAM 0x80A00000 (small kernel+initramfs, bootz'd after detach)
#   rootfs    -> RAM 0x82000000 (raw jffs2, ~22 MB; the init streams it from
#                /dev/mem into /dev/mtd6 with exact progress on the panel)
#
# After detach U-Boot bootz's the installer kernel; it writes root0 (the
# A-slot jffs2, mainline bch4 -- the proven jump-84 flow), shows the steel
# installer screen and reboots. boot0 is still empty/invalid, so the device
# returns to this same DFU window for stage 3.
#
# Usage: stage2.sh <installer-kernel> <j49-image.jffs2>
set -euo pipefail
KERNEL="${1:?usage: stage2.sh <installer-kernel> <j49-image.jffs2>}"
ROOTFS="${2:?usage: stage2.sh <installer-kernel> <j49-image.jffs2>}"
VIDPID="0451:df10"

echo "[*] waiting for the installer U-Boot DFU gadget ($VIDPID) ..."
for _ in $(seq 1 120); do
    lsusb | grep -qi "$VIDPID" && break
    sleep 1
done
lsusb | grep -qi "$VIDPID" || { echo "gadget $VIDPID never appeared" >&2; exit 1; }

echo "[*] dfu-util alts:"
dfu-util -d "$VIDPID" -l | sed 's/^/    /'

echo "[*] installer kernel -> RAM alt 'installer' (0x80A00000)"
dfu-util -d "$VIDPID" -a installer -D "$KERNEL"

SZ=$(( $(stat -L -c %s "$ROOTFS") >> 20 ))
echo "[*] rootfs staging -> RAM alt 'rootfs' (0x82000000), ${SZ} MiB"
dfu-util -d "$VIDPID" -a rootfs -D "$ROOTFS"

echo "[*] detach -> U-Boot bootz's the installer kernel"
dfu-util -d "$VIDPID" -a installer -e || true
echo "[*] the panel shows the INSTALLER screen: ERASE -> WRITE ->"
echo "    VERIFY -> DONE; the machine reboots into U-Boot afterwards."
