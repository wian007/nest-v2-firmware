#!/bin/sh
# run-off.sh <seconds> <label>: heating off for the test, suspend test, heating back as it was.
F=/var/lib/j49/thermostat
setk() { grep -v "^$1=" $F > $F.t; echo "$1=$2" >> $F.t; mv $F.t $F; }
MODE=$(sed -n 's/^mode=//p' $F)
setk mode off
i=0; until grep -q "^relay=0" /run/bpd/heat && grep -q "^demand=0" /run/bpd/heat || [ $i -ge 40 ]; do usleep 500000; i=$((i+1)); done
sh /run/susp-test.sh "$1" "$2"
setk mode "$MODE"
echo "mode restored to $MODE" >> /var/lib/j49/susp.log
