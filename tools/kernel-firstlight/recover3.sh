#!/usr/bin/env bash
# After realjump3.sh: wait for stock to come back (HW watchdog -> U-Boot -> stock;
# nlsleepd is disabled so it stays up), re-push memdump (/media/scratch is wiped at
# boot), pull the 1 MiB post-mortem window and decode it offline.
SP="$(cd "$(dirname "$0")" && pwd)"; HOST=192.168.1.50
MYSERIAL="${NEST_SERIAL:?set NEST_SERIAL to the device serial# (fw_printenv serial#); kept out of git}"
LOG="$SP/recover3.log"; DUMP="$SP/postmortem-$(date +%H%M%S).bin"
DEC="$(dirname "$0")/decode-postmortem.py"
WIN=0x83e00000; WINLEN=0x100000
export SSH_ASKPASS="$SP/askpass.sh" SSH_ASKPASS_REQUIRE=force DISPLAY=:0
: > "$LOG"; log(){ echo "[$(date +%H:%M:%S)] $*" | tee -a "$LOG"; }
SO=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=5 -o LogLevel=ERROR
    -o PreferredAuthentications=password -o PubkeyAuthentication=no -o NumberOfPasswordPrompts=1)
qsh(){ setsid -w ssh "${SO[@]}" root@$HOST "$1" 2>/dev/null; }

log "waiting for stock to return (expect ~2-3 min: watchdog ~60s + U-Boot + stock boot)"
t0=$(date +%s); end=$((t0+1800)); n=0
while [ "$(date +%s)" -lt "$end" ]; do
  OUT=$(qsh 'echo ALIVE serial=$(/sbin/fw_printenv serial# 2>/dev/null | cut -d= -f2) up=$(cut -d. -f1 /proc/uptime) nlsleepd=$(pidof nlsleepd || echo GONE)')
  if echo "$OUT" | grep -q "serial=$MYSERIAL"; then
    log "BACK after $(( $(date +%s)-t0 ))s: $OUT"
    break
  fi
  n=$((n+1)); [ $((n%20)) -eq 0 ] && log "...still down ($(( $(date +%s)-t0 ))s)"
  sleep 3
done
echo "$OUT" | grep -q ALIVE || { log "device did not return in 30 min"; exit 1; }

setsid -w ssh "${SO[@]}" root@$HOST "cat > /media/scratch/memdump && chmod +x /media/scratch/memdump && echo PUSHOK" < "$SP/memdump" 2>/dev/null | grep -q PUSHOK || { log "memdump push failed"; exit 1; }
qsh "/media/scratch/memdump $WIN $WINLEN" > "$DUMP"
log "dump: $DUMP ($(stat -c%s "$DUMP") bytes)"
qsh 'echo "cmdline: $(cat /proc/cmdline)"; dmesg | grep -iE "reset|reboot|wdt|watchdog" | head -5' | tee -a "$LOG"
python3 "$DEC" "$DUMP" | tee -a "$LOG"
