#!/bin/sh
# quiet-suspend.sh <quiet seconds> <wake mask hex> <rtc safety seconds>
# The whole chain: backplate quiet (through bpd), head unit suspended, backplate wakes it.
# Needs susp-test.sh next to it in /run. Refuses with heat demand on (susp-test.sh does).
echo enabled > /sys/devices/platform/68000000.ocp/49020000.serial/tty/ttyS2/power/wakeup
echo "$1 $2" > /run/bpd/quiet-request
usleep 3000000
sh /run/susp-test.sh "$3" "quiet $1 s mask $2, then suspend; expect a wake by the backplate after about $(( $1 - 3 )) s"
