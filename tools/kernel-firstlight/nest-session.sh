#!/bin/sh
# nest-session.sh -- guard for a kexec TEST session of the Yocto image (never installed to NAND).
# Runs as nest-session.service. Every 2 min it writes a one-line health report to /dev/kmsg
# (lands in the DEBUG_LL ring / ramoops for the post-mortem read from stock), and after
# HOLD seconds it warm-reboots so stock comes back on its own. `touch /run/nest-hold`
# over SSH extends the hold to 3x HOLD.
# HOLD must stay under the backplate's ~29 min cold reset (hits when bpd is not feeding it),
# so that a session that lost bpd still ends in a WARM reboot with the logs intact.
HOLD=${NEST_SESSION_HOLD:-1500}
k() { echo "NEST-SESSION: $*" > /dev/kmsg; }
up() { cut -d. -f1 /proc/uptime; }
: > /run/nest-session.started   # tells squash-init's deadman that systemd got this far
report() {
  k "t=$(up) $(grep -E '^(MemFree|MemAvailable)' /proc/meminfo | tr -s ' \n' ' ')\
failed=$(systemctl list-units --state=failed --no-legend --plain 2>/dev/null | awk '{print $1}' | tr '\n' ',')\
 ip=$(ip -4 -o addr show wlan0 2>/dev/null | awk '{print $4}')\
 wpa=$(wpa_cli -i wlan0 status 2>/dev/null | sed -n 's/^wpa_state=//p')\
 ssh_listen=$(grep -c ':0016 00000000:0000 0A' /proc/net/tcp 2>/dev/null)\
 load=$(cut -d' ' -f1-3 /proc/loadavg)"
}
t0=$(up)
k "start hold=${HOLD}s (touch /run/nest-hold -> $((HOLD * 3))s) $(systemctl is-system-running 2>/dev/null)"
n=0
while :; do
  [ $((n % 4)) -eq 0 ] && report
  if [ $n -eq 2 ]; then
    k "units: $(systemctl list-units --no-legend --plain --type=service 2>/dev/null | awk '{print $1"="$3}' | tr '\n' ' ')"
    journalctl -b -p warning --no-pager -o short-monotonic 2>/dev/null | tail -25 | while read -r l; do k "journal: $l"; done
  fi
  limit=$HOLD; [ -e /run/nest-hold ] && limit=$((HOLD * 3))
  [ $(( $(up) - t0 )) -ge $limit ] && break
  n=$((n + 1)); sleep 30
done
k "hold expired at t=$(up) -> warm reboot to stock"
sync; echo b > /proc/sysrq-trigger
sleep 10; reboot -f
