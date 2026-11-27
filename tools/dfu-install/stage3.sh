#!/usr/bin/env bash
# stage3.sh -- session 3: write the kernel uImage into boot0 with THIS
# U-Boot's own ECC (DFU nand alt, nand_write_skip_bad under the hood),
# then make the device reboot into the installed chain.
#
# Why the junk write to the RAM alt: DRAM survives a warm reset, so after
# stage 2 the installer kernel is still sitting at 0x80A00000. U-Boot's
# bootcmd is `nandboot || usbinstall`, and usbinstall ends in
# `bootz 0x80A00000 || reset` -- without the junk write the device would
# boot the stale installer kernel again (install loop forever). Four zero
# bytes into the RAM alt kill the zImage magic, the bootz fails, the
# script resets, and nandboot now succeeds.
#
# Usage: stage3.sh <kernel-uimage>
#   The uImage must be built with load=entry=0x80008000 (the 2026-11-04
#   root-cause lesson: bootm then copies instead of taking the XIP path).
set -euo pipefail
IMG="${1:?usage: stage3.sh <kernel-uimage>}"
VIDPID="0451:df10"
JUNK="$(mktemp /tmp/opencode/nest-dfu-junk.XXXXXX)"
trap 'rm -f "$JUNK"' EXIT
printf '\0\0\0\0' > "$JUNK"

echo "[*] waiting for the installer U-Boot DFU gadget ($VIDPID) ..."
for _ in $(seq 1 120); do
    lsusb | grep -qi "$VIDPID" && break
    sleep 1
done
lsusb | grep -qi "$VIDPID" || { echo "gadget $VIDPID never appeared" >&2; exit 1; }

echo "[*] writing $IMG to the 'boot0' nand alt (0x400000, 5 MB window)"
dfu-util -d "$VIDPID" -a boot0 -D "$IMG"

echo "[*] poisoning the stale RAM alt so bootz cannot boot stage 2 again"
dfu-util -d "$VIDPID" -a installer -D "$JUNK"

echo "[*] detach -> u-boot bootz fails -> reset -> nandboot boots the OS"
dfu-util -d "$VIDPID" -e
