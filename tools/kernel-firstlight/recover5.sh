#!/usr/bin/env bash
# Resumable, window-friendly post-mortem pull (stock SSH windows can be ~2 min):
#  * wait for our device (STOP nlsleepd on contact)
#  * push memdump only if missing (md5-verified, retried)
#  * dump the 1 MiB window to /media/scratch/pm.bin ON the device (retried)
#  * pull only what the decoder needs: ramoops chunks 0-1 (128 KiB; more if the
#    ramoops header says the text is longer) + ring chunk 8 (64 KiB), 64 KiB each,
#    size-verified and retried; everything else is zero-filled locally.
# Usage: recover5.sh OUT.bin   (env WAIT=seconds, default 7200)
SP="$(cd "$(dirname "$0")" && pwd)"; HOST=192.168.1.50
OUT="${1:-$SP/postmortem-$(date +%H%M%S).bin}"; LOG="$SP/recover5.log"
WIN=0x83e00000; WINLEN=0x100000; CH=65536
export SSH_ASKPASS="$SP/askpass.sh" SSH_ASKPASS_REQUIRE=force DISPLAY=:0
log(){ echo "[$(date +%T)] $*" | tee -a "$LOG"; }
SO=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=5 -o ServerAliveInterval=5 -o ServerAliveCountMax=3 -o LogLevel=ERROR
    -o PreferredAuthentications=password -o PubkeyAuthentication=no -o NumberOfPasswordPrompts=1)
qsh(){ timeout -s KILL "${2:-40}" setsid -w ssh "${SO[@]}" root@$HOST "$1" 2>/dev/null; }
MD5MD=$(md5sum < "$SP/memdump" | cut -c1-32)

# The stock SSH window right after a boot lasts ~2.5 min and we reach it late, so the
# poll itself carries memdump on stdin and performs the dump: first contact == dump done.
POLL="kill -STOP \$(pidof nlsleepd) 2>/dev/null; [ \"\$(md5sum /media/scratch/memdump 2>/dev/null | cut -c1-32)\" = $MD5MD ] || { cat > /media/scratch/memdump; chmod +x /media/scratch/memdump; }; /media/scratch/memdump $WIN $WINLEN > /media/scratch/pm.bin; echo ALIVE serial=\$(/sbin/fw_printenv serial# 2>/dev/null | cut -d= -f2) up=\$(cut -d. -f1 /proc/uptime) size=\$(stat -c%s /media/scratch/pm.bin 2>/dev/null) md5=\$(md5sum /media/scratch/pm.bin 2>/dev/null | cut -c1-32)"
t0=$(date +%s); end=$((t0+${WAIT:-7200})); n=0
while [ "$(date +%s)" -lt "$end" ]; do
  OUT1=$(timeout -s KILL 45 setsid -w ssh "${SO[@]}" root@$HOST "$POLL" < "$SP/memdump" 2>/dev/null)
  if echo "$OUT1" | grep -q "serial=${NEST_SERIAL:?set NEST_SERIAL}"; then log "BACK: $OUT1"; break; fi
  n=$((n+1)); [ $((n%30)) -eq 0 ] && log "...waiting ($(( $(date +%s)-t0 ))s)"; sleep 3
done
echo "$OUT1" | grep -q ALIVE || { log "device did not return"; exit 1; }
R=""
echo "$OUT1" | grep -q "size=1048576" && R="$(echo "$OUT1" | sed 's/.*size=//; s/ md5=/\n/')"
for i in 1 2 3; do
  echo "$R" | grep -q '^1048576$' && break
  R=$(qsh "/media/scratch/memdump $WIN $WINLEN > /media/scratch/pm.bin && stat -c%s /media/scratch/pm.bin && md5sum /media/scratch/pm.bin" 60)
  log "re-dump try $i: $(echo $R)"
done
echo "$R" | grep -q '^1048576$' || { log "on-device dump FAILED"; exit 1; }
log "on-device dump ok: $(echo $R)"

pull(){ # chunk index -> file $OUT.c$idx (64 KiB, verified)
  local c=$1 f="$OUT.c$1"
  [ -f "$f" ] && [ "$(stat -c%s "$f")" = "$CH" ] && return 0
  for try in 1 2 3 4 5 6 7 8; do
    qsh "dd if=/media/scratch/pm.bin bs=$CH skip=$c count=1 2>/dev/null" 40 > "$f.tmp"
    if [ "$(stat -c%s "$f.tmp")" = "$CH" ]; then mv "$f.tmp" "$f"; return 0; fi
    sleep 2
  done
  log "chunk $c failed"; return 1
}
pull 0 || exit 1
# ramoops header: u32 sig, start, size  -> how many 64 KiB chunks hold text
RSZ=$(python3 -c "import sys;d=open('$OUT.c0','rb').read();print(int.from_bytes(d[8:12],'little'))")
NEEDED=$(( (RSZ + 12 + CH - 1) / CH )); [ "$NEEDED" -lt 2 ] && NEEDED=2; [ "$NEEDED" -gt 4 ] && NEEDED=4
log "ramoops size field=$RSZ -> pulling $NEEDED ramoops chunks + ring"
for c in $(seq 1 $((NEEDED-1))) 8 9; do pull $c || exit 1; done   # 9 = raw CM/PRM/I2C1 copies at +0x90000
python3 - "$OUT" "$CH" <<'PY'
import sys,os
out,ch=sys.argv[1],int(sys.argv[2]); buf=bytearray(0x100000)
for c in range(16):
    f=f"{out}.c{c}"
    if os.path.exists(f): buf[c*ch:(c+1)*ch]=open(f,'rb').read()
open(out,'wb').write(buf)
PY
log "assembled $OUT ($(stat -c%s "$OUT") bytes; device pm.bin md5 $(echo "$R" | tail -1 | cut -c1-32))"
qsh 'kill -CONT $(pidof nlsleepd) 2>/dev/null; echo "cmdline: $(cat /proc/cmdline)"; dmesg | grep -iE "last boot|power up reset" | head -3' 20 | tee -a "$LOG"   # resume stock sleep management: leaving nlsleepd STOPped makes stock misbehave
rm -f "$OUT".c*
echo "$OUT"
