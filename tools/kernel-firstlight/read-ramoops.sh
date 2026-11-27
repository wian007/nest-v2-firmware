#!/usr/bin/env bash
# After a logjump.sh + watchdog recovery, read the modern kernel's ramoops
# console log back from the (now running) stock kernel via /dev/mem.
#
# Usage: read-ramoops.sh <host> <workdir>
#   workdir must contain the cross-built ARM reader at tools output, or pass it
#   as $3. Build the reader with build-reader.sh.
set -euo pipefail
HOST="${1:?usage: read-ramoops.sh <host> <workdir> [reader-bin]}"
WORK="${2:?need workdir}"
READER="${3:-$WORK/ramoops-read}"
S=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=8)
[ -f "$READER" ] || { echo "missing reader binary $READER (run build-reader.sh)"; exit 1; }

echo "[*] pushing reader to /media/scratch/ramoops-read"
ssh "${S[@]}" root@"$HOST" \
  "cat > /media/scratch/ramoops-read && chmod +x /media/scratch/ramoops-read" < "$READER"

echo "[*] reading ramoops console (0x83f00000) via /dev/mem ..."
ssh "${S[@]}" root@"$HOST" '/media/scratch/ramoops-read'
