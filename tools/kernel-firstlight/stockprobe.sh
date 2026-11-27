#!/usr/bin/env bash
# Run during a stock SSH window: capture what stock leaves in the GPIO banks and
# pad configuration (the reference for ti,no-reset-on-init / pinctrl in the DTS)
# plus the 2.6.37 debugfs views (gpio owners with labels, omap_mux board table).
SP="$(cd "$(dirname "$0")" && pwd)"; HOST=192.168.1.50; MD="$SP/memdump"
export SSH_ASKPASS="$SP/askpass.sh" SSH_ASKPASS_REQUIRE=force DISPLAY=:0
SO=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=6 -o LogLevel=ERROR
    -o PreferredAuthentications=password -o PubkeyAuthentication=no -o NumberOfPasswordPrompts=1)
qsh(){ setsid -w ssh "${SO[@]}" root@$HOST "$1" 2>/dev/null; }
push(){ setsid -w ssh "${SO[@]}" root@$HOST "cat > $2 && chmod +x $2 && echo PUSHOK:$2" < "$1" 2>/dev/null; }
T=$(date +%H%M%S)
OUT=$(qsh 'echo ALIVE serial=$(/sbin/fw_printenv serial# 2>/dev/null | cut -d= -f2) up=$(cut -d. -f1 /proc/uptime)')
echo "$OUT" | grep -q "serial=${NEST_SERIAL:?set NEST_SERIAL to the device serial}" || { echo "not our device / unreachable: $OUT"; exit 1; }
echo "$OUT"
push "$MD" /media/scratch/memdump | grep -q PUSHOK || { echo "memdump push failed"; exit 1; }
# GPIO banks: iclk on in stock (CM_ICLKEN_WKUP bit3, CM_ICLKEN_PER bits 13-17), safe to read.
qsh "for b in 0x48310000 0x49050000 0x49052000 0x49054000 0x49056000 0x49058000; do /media/scratch/memdump \$b 0x200; done" > "$SP/stock-gpio-$T.bin"
echo "gpio banks: $(stat -c%s "$SP/stock-gpio-$T.bin") bytes (expect 3072)"
# Pad configuration: memdump needs page-aligned addresses and the SCM page bus-faults
# in places (6.6 session), so read the 2.6.37 omap_mux debugfs instead (one file per pad,
# "name 0xMODE ..." text) -- safe, no /dev/mem.
qsh 'mount -t debugfs none /sys/kernel/debug 2>/dev/null; cd /sys/kernel/debug/omap_mux 2>/dev/null && for f in *; do [ -f "$f" ] && echo "$f: $(cat $f | tr "\n" " ")"; done' > "$SP/stock-omapmux-$T.txt"
echo "omap_mux: $(wc -l < "$SP/stock-omapmux-$T.txt") pads"
qsh 'mount -t debugfs none /sys/kernel/debug 2>/dev/null; echo "== /sys/kernel/debug/gpio"; cat /sys/kernel/debug/gpio; echo "== omap_mux"; ls /sys/kernel/debug/omap_mux/ 2>&1 | head; cat /sys/kernel/debug/omap_mux/board 2>/dev/null; echo "== /sys/class/gpio"; ls /sys/class/gpio; for g in /sys/class/gpio/gpio[0-9]*; do echo "$g $(cat $g/direction) $(cat $g/value)"; done; echo "== platform devices"; ls /sys/devices/platform/ /sys/devices/platform/omap/ 2>/dev/null | tr "\n" " "; echo; echo "== interrupts"; cat /proc/interrupts' > "$SP/stock-gpio-$T.txt"
echo "debugfs text: $(wc -l < "$SP/stock-gpio-$T.txt") lines"; grep -c 'gpio-' "$SP/stock-gpio-$T.txt"
