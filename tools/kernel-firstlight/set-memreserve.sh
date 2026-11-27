#!/usr/bin/env bash
# Reserve the top 1 MB of RAM from the STOCK kernel so the modern kernel's
# ramoops log (at 0x83f00000) survives a watchdog warm-reset long enough to be
# read back from stock. Done by appending `mem=63M` to the u-boot `addmodel`
# var (run by both nandboot0 and nandboot1) via fw_setenv — one leaf variable,
# not the boot command itself, and fully reversible.
#
# Usage: set-memreserve.sh <host> [--revert]
# Credentials via ssh-agent/key/SSH_ASKPASS. Verify with fw_printenv before any
# jump; this only takes effect on the NEXT boot.
set -euo pipefail
HOST="${1:?usage: set-memreserve.sh <host> [--revert]}"
MODE="${2:-}"
S=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=8)

cur=$(ssh "${S[@]}" root@"$HOST" 'fw_printenv addmodel 2>/dev/null')
echo "[*] current: $cur"
val=${cur#addmodel=}

if [ "$MODE" = "--revert" ]; then
  new=$(printf '%s' "$val" | sed 's/ *mem=63M//g')
  ssh "${S[@]}" root@"$HOST" "fw_setenv addmodel '$new'"
  echo "[*] reverted -> $(ssh "${S[@]}" root@"$HOST" 'fw_printenv addmodel')"
  exit 0
fi

case "$val" in
  *mem=63M*) echo "[*] already reserved: $val"; exit 0;;
esac
new="$val mem=63M"
echo "[*] setting addmodel -> $new"
ssh "${S[@]}" root@"$HOST" "fw_setenv addmodel '$new'"
echo "[*] verify: $(ssh "${S[@]}" root@"$HOST" 'fw_printenv addmodel')"
echo "[!] takes effect on the next boot. Sanity-check the value above before jumping."
