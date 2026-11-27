#!/bin/sh
# quiet-test.sh <mask-hex-le> <seconds> <listen>: put the backplate in its quiet mode and listen.
# bpd is stopped for the test (it would talk to the backplate and end the quiet mode at once);
# stopping it switches the heating off. It is started again at the end.
MASK=$1; SECS=$2; LISTEN=$3
systemctl stop bpd
usleep 1500000
echo "--- 5 s of normal stream:"
bpd sniff /dev/ttyS2 --seconds 5 2>&1 | grep -c "type" | sed 's/^/frames: /'
bpd send /dev/ttyS2 a4 $MASK >/dev/null 2>&1
bpd send /dev/ttyS2 a1 $(printf '%02x%02x' $((SECS & 255)) $((SECS >> 8))) >/dev/null 2>&1
echo "--- quiet: mask $MASK, $SECS s; listening $LISTEN s (nothing is sent):"
bpd sniff /dev/ttyS2 --seconds $LISTEN 2>&1 | cut -c1-110 | tail -n 40
systemctl start bpd
