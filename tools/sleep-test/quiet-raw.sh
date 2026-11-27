#!/bin/sh
# quiet-raw.sh <mask-hex-le> <seconds> <listen>: put the backplate in quiet mode, send nothing,
# and keep the raw bytes it sends. Files are on flash and synced every second, because the
# backplate resets the head unit when its wake-up is not answered.
#   /var/lib/j49/quiet.raw    the bytes
#   /var/lib/j49/quiet.sizes  "<second> <bytes so far> <uptime>"
# bpd is stopped for the test (heating off) and started again at the end.
MASK=$1; SECS=$2; LISTEN=$3
D=/var/lib/j49
systemctl stop bpd
usleep 1500000
bpd send /dev/ttyS2 a4 $MASK >/dev/null 2>&1
bpd send /dev/ttyS2 a1 $(printf '%02x%02x' $((SECS & 255)) $((SECS >> 8))) >/dev/null 2>&1
: > $D/quiet.raw; echo "start $(date '+%T') mask=$MASK secs=$SECS" > $D/quiet.sizes
cat /dev/ttyS2 > $D/quiet.raw &
C=$!
i=0; while [ $i -lt $LISTEN ]; do usleep 1000000; i=$((i+1)); echo "$i $(stat -c %s $D/quiet.raw) $(cut -d' ' -f1 /proc/uptime)" >> $D/quiet.sizes; sync; done
kill $C
echo "end $(date '+%T'): still running, bpd started again" >> $D/quiet.sizes; sync
systemctl start bpd
