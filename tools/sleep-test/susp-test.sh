#!/bin/sh
# susp-test.sh [seconds] [label]: suspend to RAM once and log what woke the J49.
# The RTC alarm is the safety net: it wakes the board after [seconds] (default 120) when nothing
# else does. Run it detached (the SSH session dies with WiFi):
#   setsid sh /run/susp-test.sh 120 "backplate stream" >/dev/null 2>&1 &
# Log: /var/lib/j49/susp.log. Never run it while the heating is on: nobody controls it in suspend.
S=${1:-120}; LABEL=${2:-}
L=/var/lib/j49/susp.log
irqs() { grep -E "49020000.serial|rtc0|pwrbutton|adbs|wl12|gpio" /proc/interrupts | awk '{printf "%s%s=%s ", $NF, ($NF ~ /wakeup/ ? "" : ""), $2}'; }
if grep -q "^demand=1" /run/bpd/heat 2>/dev/null; then echo "== $(date '+%F %T') REFUSED: heat demand is on" >> $L; exit 1; fi
echo "== $(date '+%F %T') suspend test: alarm ${S}s; $LABEL" >> $L
echo "before: $(irqs)" >> $L
echo 0 > /sys/class/rtc/rtc0/wakealarm; echo +$S > /sys/class/rtc/rtc0/wakealarm
dmesg -c > /dev/null
sync
T0=$(date +%s)
echo mem > /sys/power/state; RC=$?
T1=$(date +%s)
echo "rc=$RC asleep=$((T1-T0))s of ${S}s wakeup_irq=$(cat /sys/power/pm_wakeup_irq 2>/dev/null)" >> $L
echo "after:  $(irqs)" >> $L
dmesg | grep -iE "PM:|wake|suspend|resume|Powerdomain|didn't|fail|error" | cut -c1-150 | tail -n 25 >> $L
echo 0 > /sys/class/rtc/rtc0/wakealarm
sync
