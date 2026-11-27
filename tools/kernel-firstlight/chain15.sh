#!/usr/bin/env bash
# jump N chain: realjump3 (retry until a window) -> recover5 (window-friendly partial pull) -> decode + I2C summary
SP="$(cd "$(dirname "$0")" && pwd)"; cd "$SP"; N=${1:-15}
for i in $(seq 1 ${ATTEMPTS:-120}); do
  bash realjump3.sh > jump$N.out 2>&1; rc=$?
  grep -q JUMPING jump$N.out && break
  echo "[$(date +%T)] attempt $i rc=$rc: $(tail -1 jump$N.out)"; sleep 25
done
grep -q JUMPING jump$N.out || { echo "NO JUMP"; exit 1; }
echo "[$(date +%T)] JUMPED; polling for stock right away (post-boot WiFi window is ~2 min, next one ~38 min later)"
WAIT=5400 bash recover5.sh "$SP/postmortem-$N.bin" > recover$N.out 2>&1; rc=$?
echo "recover rc=$rc: $(tail -1 recover5.log)"
python3 "$(dirname "$0")/decode-postmortem.py" "$SP/postmortem-$N.bin" > decode$N.txt
# ramoops half only (ring duplicates it)
awk '/^== ramoops/{p=1} p' decode$N.txt > decode$N-ramoops.txt
echo "twl failures: $(grep -c 'twl: .*failed\|Arbitration lost\|controller timed out' decode$N-ramoops.txt) | STAGE3 ticks: $(grep -c 'STAGE3 still' decode$N-ramoops.txt) | oops: $(grep -c 'Oops\|Unable to handle\|Internal error\|Kernel panic' decode$N-ramoops.txt)"
grep -E 'STAGE2 (i2c|I2C|PRM|regulators)|NEST-FL: I2C|experiment|unbound|STAGE[5-7]|verdict' decode$N-ramoops.txt | sed 's/.*NEST-FL: //' | head -60
