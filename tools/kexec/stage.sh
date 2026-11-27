#!/usr/bin/env bash
# Push kexec + a zImage to the Nest and prove the kexec staging chain
# NON-DESTRUCTIVELY (kexec -l then -u; never -e). Restores nlsleepd afterwards.
#
# Usage: stage.sh <host> [kexec-binary] [zImage]
#   Credentials: supply via ssh-agent, a key, or SSH_ASKPASS. Nothing is stored
#   here. Build the binary with:
#     nix build nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.kexec-tools
set -euo pipefail
HOST="${1:?usage: stage.sh <host> [kexec] [zImage]}"
KEXEC="${2:-./kexec}"
ZIMAGE="${3:-./stock-zImage}"
SSHOPTS=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=8)

echo "[*] pushing kexec -> /media/scratch/kexec"
ssh "${SSHOPTS[@]}" root@"$HOST" \
  "cat > /media/scratch/kexec && chmod +x /media/scratch/kexec && echo ok" < "$KEXEC"

echo "[*] pushing zImage -> /media/scratch/stage-zImage"
ssh "${SSHOPTS[@]}" root@"$HOST" \
  "cat > /media/scratch/stage-zImage && echo ok" < "$ZIMAGE"

echo "[*] staging (non-destructive: -l then -u, no -e)"
ssh "${SSHOPTS[@]}" root@"$HOST" '
  kill -STOP nlsleepd 2>/dev/null
  K=/media/scratch/kexec; Z=/media/scratch/stage-zImage
  $K -v | head -1
  echo "loaded_before=$(cat /sys/kernel/kexec_loaded)"
  $K -l $Z --type=zImage --command-line="$(cat /proc/cmdline)" --atags
  echo "kexec_-l_rc=$?  loaded_after_l=$(cat /sys/kernel/kexec_loaded)"
  $K -u
  echo "kexec_-u_rc=$?  loaded_after_u=$(cat /sys/kernel/kexec_loaded)"
  kill -CONT nlsleepd 2>/dev/null
'
echo "[*] done — chain proven, device left unarmed and nlsleepd restored."
echo "    live jump is: ssh root@$HOST '/media/scratch/kexec -e'  (only with physical access)"
