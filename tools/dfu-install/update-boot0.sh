#!/usr/bin/env bash
# wait for the REAL DFU window (dfu-util lists the boot0 alt), then write boot0 and leave
IMG="$1"; J=$(mktemp); printf '\0\0\0\0' > "$J"
for i in $(seq 1 200); do dfu-util -l 2>/dev/null | grep -q 'name="boot0"' && break; sleep 1; done
dfu-util -l 2>/dev/null | grep -q 'name="boot0"' || { echo "no DFU device with a boot0 partition"; exit 1; }
echo "DFU-venster open na ${i}s"; dfu-util -l 2>/dev/null | grep -o 'alt=[0-9]*, name="[^"]*"' | tr "\n" " "; echo
dfu-util -d 0451:df10 -a boot0 -D "$IMG" 2>&1 | tail -n 4
dfu-util -d 0451:df10 -a installer -D "$J" 2>&1 | tail -n 2
dfu-util -d 0451:df10 -a installer -e 2>&1 | tail -n 1
