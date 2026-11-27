#!/usr/bin/env bash
# Host-side visibility for the kexec live jump. The Nest has no wired serial
# console and no mainline display driver, but it hangs off this host by USB.
# When the modern kernel boots, MUSB + g_serial enumerate a NEW USB device
# (CDC-ACM) distinct from the stock gadget (2464:0002), giving us /dev/ttyACMx.
#
# Run this in one terminal, then fire `stage.sh <host> <workdir> --exec` in
# another. Watch for: stock device drops -> a CDC-ACM gadget appears -> shell.
set -u
DUR="${1:-120}"
echo "[*] baseline USB:"; lsusb | grep -iE "2464|1d6b:0104|0525|NetChip|gadget" || true
base_acm=$(ls /dev/ttyACM* 2>/dev/null || true)
echo "[*] baseline ttyACM: ${base_acm:-none}"
echo "[*] watching ${DUR}s for the modern-kernel gadget ..."
end=$(( $(date +%s) + DUR ))
while [ "$(date +%s)" -lt "$end" ]; do
  now=$(ls /dev/ttyACM* 2>/dev/null || true)
  new=$(comm -13 <(printf '%s\n' $base_acm | sort) <(printf '%s\n' $now | sort) 2>/dev/null | tr -d ' ')
  if [ -n "$new" ]; then
    echo "[+] NEW serial gadget: $new  — modern kernel reached userspace!"
    echo "[+] opening a console (115200 8N1). Ctrl-A then k/x to quit screen/minicom,"
    echo "    or just: cat $new  (and in another shell: echo cmd > $new)"
    command -v picocom >/dev/null && exec picocom -b 115200 "$new"
    command -v screen  >/dev/null && exec screen "$new" 115200
    echo "[*] no picocom/screen; raw cat follows:"; exec cat "$new"
  fi
  sleep 1
done
echo "[*] no new gadget within ${DUR}s."
echo "    If stock (2464:0002) also vanished and nothing came back, the kernel"
echo "    likely hung early — power-cycle the device (NAND still boots stock)."
