#!/usr/bin/env bash
# Off-base loop: device is on USB at the host. Wait for a stock SSH window, run the
# stock probe once (optional), jump with realjump3.sh, then wait for the CDC-ACM
# console and snapshot the new kernel. Env: NEST_ZIMAGE/NEST_CL (realjump3), ATTEMPTS,
# PROBE=1 runs stockprobe.sh first, TAG names the snapshot files.
SP="$(cd "$(dirname "$0")" && pwd)"; TAG="${TAG:-usb$(date +%H%M)}"
for i in $(seq 1 "${ATTEMPTS:-200}"); do
  if ping -c1 -W1 192.168.1.50 >/dev/null 2>&1; then
    [ "${PROBE:-0}" = 1 ] && { bash "$SP/stockprobe.sh" && PROBE=0; }
    bash "$SP/realjump3.sh" 2>&1 | tee "$SP/usbjump-$TAG.log" | grep -q '^JUMPING' && break
  fi
  sleep 20
done
grep -q '^JUMPING' "$SP/usbjump-$TAG.log" 2>/dev/null || { echo "no jump"; exit 1; }
echo "[$(date +%T)] jumped; waiting for /dev/ttyACM1"
for i in $(seq 1 90); do [ -e /dev/ttyACM1 ] && break; sleep 1; done
[ -e /dev/ttyACM1 ] || { echo "[$(date +%T)] NO USB CONSOLE after 90 s"; exit 2; }
# g_serial drops RX until the device side has opened ttyGS0 (the STAGE4 shell), so wait
# until an echo comes back before sending the real commands.
for i in $(seq 1 40); do
  python3 "$SP/tty.py" /dev/ttyACM1 2 "echo READY_\$((40+2))" 2>/dev/null | grep -q READY_42 && break
  [ -e /dev/ttyACM1 ] || { echo "[$(date +%T)] console vanished (kernel rebooted?)"; exit 3; }
  sleep 1
done
echo "[$(date +%T)] console answers after $i probes"
python3 "$SP/tty.py" /dev/ttyACM1 15 "" \
 "echo ==UP; uptime; cat /proc/version; ls /stage4" \
 "echo ==GPIOREGS; for g in 48310000 49050000 49052000 49054000 49056000 49058000; do echo on > /sys/bus/platform/devices/\$g.gpio/power/control; done; for g in 0x48310000 0x49050000 0x49052000 0x49054000 0x49056000 0x49058000; do printf '%s OE=%s IN=%s OUT=%s\n' \$g \$(devmem \$((g+0x34))) \$(devmem \$((g+0x38))) \$(devmem \$((g+0x3c))); done" \
 "echo ==I2C; timeout -s KILL 8 /bin/i2cprobe /dev/i2c-1 scan | tr '\n' ' '; echo; timeout -s KILL 8 /bin/i2cprobe /dev/i2c-2 scan | tr '\n' ' '; echo" \
 "echo ==PSU; for p in /sys/class/power_supply/*; do echo \$p; cat \$p/uevent 2>/dev/null; done; ls /dev/rtc* 2>&1; hwclock -r 2>&1" \
 "echo ==ERR; dmesg | grep -iE 'error|fail|warn' | grep -v NEST-FL | tail -15 | cut -c1-160" > "$SP/usbjump-$TAG.snap" 2>&1
echo "[$(date +%T)] snapshot: $SP/usbjump-$TAG.snap"; grep -A8 '^==GPIOREGS\|^==I2C\|^==PSU' "$SP/usbjump-$TAG.snap" | cut -c1-160
