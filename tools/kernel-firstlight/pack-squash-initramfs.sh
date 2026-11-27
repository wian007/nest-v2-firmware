#!/usr/bin/env bash
# pack-squash-initramfs.sh -- build the kexec initramfs that boots the real Yocto image from RAM
# (phase 3 test path): static busybox + squash-init + the j49-image squashfs-xz + a
# provisioning overlay that is copied into the writable upper layer before switch_root.
#
#   pack-squash-initramfs.sh <busybox> <rootfs.squashfs-xz> <overlay-dir> <out.cpio.gz>
#
# <overlay-dir> is a rootfs-shaped tree kept OUTSIDE git (scratchpad): the network secrets
# (etc/wpa_supplicant/wpa_supplicant-wlan0.conf, etc/dropbear/* host keys,
# root/.ssh/authorized_keys), the device NVS (lib/firmware/ti-connectivity/wl127x-nvs.bin)
# and anything test-only. nest-session.{sh,service} from this directory are added to it
# automatically (health lines to kmsg + warm reboot to stock after the hold) and enabled.
set -euo pipefail
BB=$1; SQ=$2; OV=$3; OUT=$4
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT

mkdir -p "$ROOT"/{bin,dev,proc,sys,lower,rw,newroot,overlay}
cp "$BB" "$ROOT/bin/busybox"; chmod +x "$ROOT/bin/busybox"
for a in sh mount umount losetup switch_root cp find ls cat cut grep sed awk sleep sync tr wc uname mkdir echo; do
  ln -sf busybox "$ROOT/bin/$a"
done
install -m 755 "$HERE/squash-init" "$ROOT/init"
cp "$SQ" "$ROOT/rootfs.squashfs"

# provisioning overlay = caller's tree + the session guard
cp -a "$OV/." "$ROOT/overlay/"
# Firmware the kernel requests BEFORE switch_root must also sit in the initramfs root:
# wl12xx asks for the NVS at ~3.7 s (probe fails for good without it -> no wlan0, jump 72)
# and cfg80211 for regulatory.db at ~4 s. Mirror the overlay's lib/firmware there.
if [ -d "$OV/lib/firmware" ]; then mkdir -p "$ROOT/lib"; cp -a "$OV/lib/firmware" "$ROOT/lib/"; fi
install -D -m 755 "$HERE/nest-session.sh" "$ROOT/overlay/usr/local/sbin/nest-session.sh"
install -D -m 644 "$HERE/nest-session.service" "$ROOT/overlay/etc/systemd/system/nest-session.service"
mkdir -p "$ROOT/overlay/etc/systemd/system/multi-user.target.wants"
ln -sf /etc/systemd/system/nest-session.service \
  "$ROOT/overlay/etc/systemd/system/multi-user.target.wants/nest-session.service"
# dropbear: pubkey only (so recover5's password probe cannot mistake this for stock)
install -D -m 644 /dev/stdin "$ROOT/overlay/etc/default/dropbear" <<'EOF'
DROPBEAR_EXTRA_ARGS="-s"
EOF
chmod 700 "$ROOT/overlay/root" "$ROOT/overlay/root/.ssh" 2>/dev/null || true
chmod 600 "$ROOT"/overlay/etc/dropbear/* "$ROOT"/overlay/root/.ssh/authorized_keys \
          "$ROOT"/overlay/etc/wpa_supplicant/* 2>/dev/null || true

# cpio, everything owned by root (authorized_keys and the host keys must be)
nix shell nixpkgs#cpio nixpkgs#gzip nixpkgs#findutils --command bash -c "
  cd '$ROOT' && find . -print0 | cpio --null -o --format=newc -R 0:0 2>/dev/null | gzip -9 > '$OUT'
"
echo "=== $OUT ==="
ls -la "$OUT"
echo "squashfs: $(stat -Lc%s "$SQ") bytes; overlay files: $(find "$ROOT/overlay" ! -type d | sed "s#$ROOT/overlay/##" | tr '\n' ' ')"
