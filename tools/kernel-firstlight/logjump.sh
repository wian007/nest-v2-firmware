#!/usr/bin/env bash
# Hands-free modern-kernel first light: push + stage + LIVE kexec jump, relying
# on the stock hardware watchdog to warm-reset back to stock (~60s) whatever
# happens. The modern kernel logs to ramoops (RAM, preserved via mem=63M); read
# it back afterwards with read-ramoops.sh once stock is reachable again.
#
# PRE-REQS (checked): kexec binary already on /media/scratch (tools/kexec),
# and set-memreserve.sh has been run + verified. Do this only when you accept
# that, in the worst case, a non-clean reset leaves the device offline until you
# can physically power-cycle it (NAND still boots stock).
#
# Usage: logjump.sh <host> <workdir>
set -euo pipefail
HOST="${1:?usage: logjump.sh <host> <workdir>}"
WORK="${2:?need workdir}"
Z="$WORK/out/arch/arm/boot/zImage"
D="$WORK/out/arch/arm/boot/dts/ti/omap/omap3-nest-j49.dtb"
I="$WORK/initramfs.cpio.gz"
CL="loglevel=8 ignore_loglevel rdinit=/init"
S=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=8)
for f in "$Z" "$D" "$I"; do [ -f "$f" ] || { echo "missing $f"; exit 1; }; done

# ramoops sits in the confirmed-safe low window (0x83f00000, 256 KB), which stock
# leaves alone, so no mem=63M reservation is needed. Hold a stable shell first:
echo "[*] stabilizing (STOP nlsleepd+monit so the push doesn't race the sleep)"
ssh "${S[@]}" root@"$HOST" 'kill -STOP $(pidof nlsleepd) 2>/dev/null; kill -STOP $(pidof monit) 2>/dev/null; echo held' \
  || { echo "!! no stable window; retry when a wake window is open"; exit 1; }

echo "[*] pushing modern kernel/dtb/initrd"
ssh "${S[@]}" root@"$HOST" "cat > /media/scratch/mk-zImage" < "$Z"
ssh "${S[@]}" root@"$HOST" "cat > /media/scratch/mk.dtb"     < "$D"
ssh "${S[@]}" root@"$HOST" "cat > /media/scratch/mk-initrd"  < "$I"

echo "[!] JUMPING now. SSH will drop; watchdog warm-resets to stock in ~1 min."
ssh "${S[@]}" root@"$HOST" '
  kill -STOP nlsleepd 2>/dev/null
  K=/media/scratch/kexec
  $K -l /media/scratch/mk-zImage --type=zImage --dtb=/media/scratch/mk.dtb \
     --initrd=/media/scratch/mk-initrd --command-line="'"$CL"'"
  echo "staged (loaded=$(cat /sys/kernel/kexec_loaded)); jumping..."
  sync; $K -e
' || echo "[*] ssh dropped (expected — the kernel jumped)."
echo "[*] wait ~2 min for stock to come back, then: read-ramoops.sh $HOST $WORK"
