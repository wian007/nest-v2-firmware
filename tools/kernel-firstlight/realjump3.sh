#!/usr/bin/env bash
# 3rd modern-kernel jump. Changes over realjump2.sh (from the 2026-10-27 dump analysis):
#  * post-mortem window moved to 0x83e00000..0x83f00000: above stock's mem=62M top,
#    below U-Boot's relocation/framebuffer footprint (U-Boot clears 0x83f03000+ on
#    every boot -- that is what ate the previous ramoops zone).
#  * kernel built with DEBUG_LL -> DRAM ring @0x83e80000 (decompressor banner +
#    earlyprintk), ramoops @0x83e00000. cmdline adds earlyprintk keep_bootcon.
#  * both zones are ZERO-FILLED before the jump (decay only clears bits, so any
#    set bit afterwards was written by the new kernel).
#  * nlsleepd is persistently disabled on the device now, so no window hunting:
#    the device is simply up.
#  * zImage-dtb-kx: header _magic_end bumped so kexec-tools does not strip the appended
#    DTB (root cause of jumps 2-4: kernel saw ATAGs only -> "invalid dtb ... machine ID").
# Still: appended DTB, --atags, mem=64M, no --dtb. Aborts cleanly if kexec -l fails.
SP="$(cd "$(dirname "$0")" && pwd)"; KB="$SP/kbuild"
LOG="$SP/realjump3.log"; HOST=192.168.1.50
C="${NEST_ZIMAGE:-$KB/zImage-dtb-kx}"; I="$KB/initramfs.cpio.gz"; K="$SP/kexec-arm"; MD="$SP/memdump"
CL="${NEST_CL:-mem=64M earlyprintk keep_bootcon panic=15 loglevel=8 ignore_loglevel rdinit=/init}"  # (clk_ignore_unused no longer needed: patches/0003 keeps sys_clkout1 = TWL HFCLKIN on; verified jump 26)
WIN=0x83e00000; WINLEN=0x100000
export SSH_ASKPASS="$SP/askpass.sh" SSH_ASKPASS_REQUIRE=force DISPLAY=:0
: > "$LOG"; log(){ echo "[$(date +%H:%M:%S)] $*" | tee -a "$LOG"; }
SO=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=6 -o LogLevel=ERROR
    -o PreferredAuthentications=password -o PubkeyAuthentication=no -o NumberOfPasswordPrompts=1)
qsh(){ setsid -w ssh "${SO[@]}" root@$HOST "$1" 2>/dev/null; }
push(){ setsid -w ssh "${SO[@]}" root@$HOST "cat > $2 && chmod +x $2 && echo PUSHOK:$2" < "$1" 2>/dev/null; }

for f in "$C" "$I" "$K" "$MD"; do [ -f "$f" ] || { log "MISSING $f"; exit 1; }; done
log "artifacts: combined=$(stat -c%s "$C") initrd=$(stat -c%s "$I") kexec=$(stat -c%s "$K") memdump=$(stat -c%s "$MD")"

OUT=$(qsh 'kill -STOP $(pidof nlsleepd) 2>/dev/null; mount | grep -q " /sys/power/state " || mount --bind /dev/null /sys/power/state 2>/dev/null; echo ALIVE serial=$(/sbin/fw_printenv serial# 2>/dev/null | cut -d= -f2) up=$(cut -d. -f1 /proc/uptime) nlsleepd=$(pidof nlsleepd || echo GONE) wd=$(pidof watchdogd)')
echo "$OUT" | grep -q "serial=${NEST_SERIAL:?set NEST_SERIAL}" || { log "not our device / unreachable: $OUT"; exit 1; }
log "device: $OUT"

log "pushing kexec / zImage-dtb / initrd / memdump..."
push "$MD" /media/scratch/memdump   | grep -q PUSHOK || { log "memdump push failed"; exit 1; }
# stock I2C1 controller + CM_CORE regs for comparison with the modern kernel's setup
# (I2C1 is runtime-idled under stock -> its registers bus-fault; pin it on first)
qsh "echo on > /sys/devices/platform/omap/omap_i2c.1/power/control; sleep 0.3; /media/scratch/memdump 0x48070000 0x1000; /media/scratch/memdump 0x48004a00 0x100; echo auto > /sys/devices/platform/omap/omap_i2c.1/power/control" > "$SP/stock-i2c1-$(date +%H%M%S).bin"
# stock CM (0x48004000, 8 KiB) + PRM (0x48306000, 8 KiB) = the bootloader/stock clock state the modern kernel must preserve
qsh "/media/scratch/memdump 0x48004000 0x2000; /media/scratch/memdump 0x48306000 0x2000" > "$SP/stock-cm-prm-$(date +%H%M%S).bin"
log "stock CM+PRM dump: $(stat -c%s "$SP"/stock-cm-prm-*.bin 2>/dev/null | tail -1) bytes"
log "stock I2C1 regs: $(od -An -tx4 -v -w28 -j 0xa4 -N 0x1c "$SP"/stock-i2c1-*.bin 2>/dev/null | tail -1) (CON PSC SCLL SCLH SYSTEST BUFSTAT @a4..)"
push "$K"  /media/scratch/kexec     | grep -q PUSHOK || { log "kexec push failed"; exit 1; }
push "$C"  /media/scratch/mk-zImage | grep -q PUSHOK || { log "combined push failed"; exit 1; }
push "$I"  /media/scratch/mk-initrd | grep -q PUSHOK || { log "initrd push failed"; exit 1; }
qsh "ls -la /media/scratch/; md5sum /media/scratch/mk-zImage" | tee -a "$LOG"; md5sum "$C" | tee -a "$LOG"

log "zero-filling post-mortem window $WIN+$WINLEN (ramoops zone + DEBUG_LL ring)"
qsh "/media/scratch/memdump $WIN $WINLEN 00; /media/scratch/memdump $WIN 0x100000 | tr -d '\\000' | wc -c" | tee -a "$LOG"

log "staging kexec -l (appended DTB, --atags) + verifying"
STG=$(qsh '
  K=/media/scratch/kexec
  $K -d -l /media/scratch/mk-zImage --type=zImage --atags \
     --initrd=/media/scratch/mk-initrd --command-line="'"$CL"'" 2>&1
  echo "stage_rc=$?"
  echo "kexec_loaded=$(cat /sys/kernel/kexec_loaded 2>/dev/null)"')
log "stage result:"; echo "$STG" | tee -a "$LOG"
if ! echo "$STG" | grep -q "kexec_loaded=1"; then
  log "!! kexec -l did NOT load -- ABORT, no jump."
  qsh '/media/scratch/kexec -u 2>/dev/null; echo unloaded'; exit 2
fi

log "LOADED. Committing: sync + kexec -e at $(date +%T). SSH drops; HW watchdog resets to stock."
timeout 25 setsid -w ssh "${SO[@]}" root@$HOST 'sync; echo JUMPING; /media/scratch/kexec -e' 2>/dev/null || true   # the session dies with the kernel; do not wait for TCP to notice
log "ssh dropped (expected). NEXT: recover3.sh (waits for stock, dumps window, decodes)."
exit 0
