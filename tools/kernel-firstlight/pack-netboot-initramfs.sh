#!/usr/bin/env bash
# pack-netboot-initramfs.sh -- the ~3 MB kexec initramfs that fetches the Yocto image over WiFi
# (netboot-init) and then boots it from RAM (squash-init). Nothing in it is big enough to
# matter next to the 20 MB kernel; the squashfs arrives later over SSH (netpush.sh).
#
#   pack-netboot-initramfs.sh <busybox> <netdir> <overlay-dir> <out.cpio.gz> [fwdir]
#
# <busybox>     static armhf busybox (nix pkgsStatic; applet list read via qemu-arm)
# <netdir>      static musl wpa_supplicant, wpa_cli, dropbear, udhcpc.script and wpa.conf
#               (scratchpad only: the SSID/PSK are a secret)
# <overlay-dir> rootfs-shaped tree copied into the image's writable layer by squash-init:
#               etc/wpa_supplicant/wpa_supplicant-wlan0.conf, etc/dropbear/* host keys,
#               root/.ssh/authorized_keys, lib/firmware/** (NVS, regulatory.db). The host
#               keys / authorized_keys / firmware are ALSO used by this initramfs itself.
set -euo pipefail
BB=$1; NET=$2; OV=$3; OUT=$4   # $5 = optional firmware tree (wl127x-fw-5-*.bin)
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
QEMU=${QEMU:-qemu-arm}

mkdir -p "$ROOT"/{bin,sbin,dev,proc,sys,etc/dropbear,root/.ssh,var/run/wpa_supplicant,tmp,usr/share/udhcpc,lower,rw,newroot,overlay}
cp "$BB" "$ROOT/bin/busybox"; chmod +x "$ROOT/bin/busybox"
for a in $("$QEMU" "$BB" --list); do ln -sf busybox "$ROOT/bin/$a"; done
install -m 755 "$HERE/netboot-init" "$ROOT/init"
install -m 755 "$HERE/squash-init" "$ROOT/init-squash"

install -m 755 "$NET/wpa_supplicant" "$ROOT/sbin/wpa_supplicant"
install -m 755 "$NET/wpa_cli" "$ROOT/bin/wpa_cli"
install -m 755 "$NET/dropbear" "$ROOT/sbin/dropbear"
install -m 755 "$NET/udhcpc.script" "$ROOT/usr/share/udhcpc/default.script"
install -m 600 "$NET/wpa.conf" "$ROOT/etc/wpa.conf"
printf 'root:x:0:0:root:/root:/bin/sh\n' > "$ROOT/etc/passwd"   # dropbear's getpwnam(root)
printf 'root:x:0:\n' > "$ROOT/etc/group"
install -m 600 "$OV"/etc/dropbear/dropbear_*_host_key "$ROOT/etc/dropbear/"
install -m 600 "$OV/root/.ssh/authorized_keys" "$ROOT/root/.ssh/authorized_keys"
chmod 700 "$ROOT/root" "$ROOT/root/.ssh"
mkdir -p "$ROOT/lib"; cp -a "$OV/lib/firmware" "$ROOT/lib/"   # NVS + regdb: requested before switch_root
# wlcore loads wl127x-fw-5-sr.bin when the interface comes up, i.e. in THIS initramfs
# (jump 75 failed right there). <fwdir> = a linux-firmware-style tree (ti-connectivity/...).
if [ -n "${5:-}" ]; then cp -a "$5/." "$ROOT/lib/firmware/"; fi

# provisioning overlay for the image = caller's tree + the session guard (see squash-init)
cp -a "$OV/." "$ROOT/overlay/"
install -D -m 755 "$HERE/nest-session.sh" "$ROOT/overlay/usr/local/sbin/nest-session.sh"
install -D -m 644 "$HERE/nest-session.service" "$ROOT/overlay/etc/systemd/system/nest-session.service"
mkdir -p "$ROOT/overlay/etc/systemd/system/multi-user.target.wants"
ln -sf /etc/systemd/system/nest-session.service \
  "$ROOT/overlay/etc/systemd/system/multi-user.target.wants/nest-session.service"
install -D -m 644 /dev/stdin "$ROOT/overlay/etc/default/dropbear" <<'EOF'
DROPBEAR_EXTRA_ARGS="-s"
EOF
chmod 700 "$ROOT/overlay/root" "$ROOT/overlay/root/.ssh" 2>/dev/null || true
chmod 600 "$ROOT"/overlay/etc/dropbear/* "$ROOT"/overlay/root/.ssh/authorized_keys \
          "$ROOT"/overlay/etc/wpa_supplicant/* 2>/dev/null || true

nix shell nixpkgs#cpio nixpkgs#gzip nixpkgs#findutils --command bash -c "
  cd '$ROOT' && find . -print0 | cpio --null -o --format=newc -R 0:0 2>/dev/null | gzip -9 > '$OUT'
"
echo "=== $OUT ==="
ls -la "$OUT"
echo "applets: $("$QEMU" "$BB" --list | wc -l); overlay files: $(find "$ROOT/overlay" ! -type d | sed "s#$ROOT/overlay/##" | tr '\n' ' ')"
