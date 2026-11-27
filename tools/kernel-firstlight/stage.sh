#!/usr/bin/env bash
# Push the modern kernel + DTB + initramfs to the Nest and either prove kexec can
# LOAD them (default, non-destructive: -l then -u) or do the LIVE jump (--exec).
# Credentials via ssh-agent/key/SSH_ASKPASS; nothing stored here.
#
# Usage: stage.sh <host> <workdir> [--exec]
#   default : push + kexec -l + kexec -u   (safe; nothing boots)
#   --exec  : push + kexec -l + kexec -e   (BOOTS the modern kernel NOW;
#             a failed boot hangs the SoC -> power-cycle. Only with presence.)
set -euo pipefail
HOST="${1:?usage: stage.sh <host> <workdir> [--exec]}"
WORK="${2:?need workdir}"
MODE="${3:-}"
Z="$WORK/out/arch/arm/boot/zImage"
D="$WORK/out/arch/arm/boot/dts/ti/omap/omap3-nest-j49.dtb"
I="$WORK/initramfs.cpio.gz"
CL="console=ttyGS0 loglevel=8 ignore_loglevel rdinit=/init"
S=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=8)

for f in "$Z" "$D" "$I"; do [ -f "$f" ] || { echo "missing $f"; exit 1; }; done
echo "[*] pushing zImage/dtb/initrd to /media/scratch ..."
ssh "${S[@]}" root@"$HOST" "cat > /media/scratch/mk-zImage"  < "$Z"
ssh "${S[@]}" root@"$HOST" "cat > /media/scratch/mk.dtb"     < "$D"
ssh "${S[@]}" root@"$HOST" "cat > /media/scratch/mk-initrd"  < "$I"

if [ "$MODE" = "--exec" ]; then
  echo "[!] LIVE JUMP — modern kernel boots now; watch the host USB (watch-usb.sh)"
  ssh "${S[@]}" root@"$HOST" '
    kill -STOP nlsleepd 2>/dev/null
    K=/media/scratch/kexec
    $K -l /media/scratch/mk-zImage --type=zImage --dtb=/media/scratch/mk.dtb \
       --initrd=/media/scratch/mk-initrd --command-line="'"$CL"'"
    echo "staged (loaded=$(cat /sys/kernel/kexec_loaded)); jumping..."
    sync; $K -e
  ' || echo "[*] ssh dropped (expected — the kernel jumped)."
else
  echo "[*] non-destructive staging proof (-l then -u) ..."
  ssh "${S[@]}" root@"$HOST" '
    kill -STOP nlsleepd 2>/dev/null
    K=/media/scratch/kexec
    echo "loaded_before=$(cat /sys/kernel/kexec_loaded)"
    $K -l /media/scratch/mk-zImage --type=zImage --dtb=/media/scratch/mk.dtb \
       --initrd=/media/scratch/mk-initrd --command-line="'"$CL"'"
    echo "kexec_-l_rc=$?  loaded_after_l=$(cat /sys/kernel/kexec_loaded)"
    $K -u; echo "kexec_-u_rc=$?  loaded_after_u=$(cat /sys/kernel/kexec_loaded)"
    kill -CONT nlsleepd 2>/dev/null
  '
  echo "[*] done — proven loadable, device left unarmed."
fi
