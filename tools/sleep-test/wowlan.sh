#!/bin/sh
# wowlan.sh [show|off]: tell the WiFi chip which packets may wake the suspended head unit.
# Needs a kernel with keep-power-in-suspend on the WL1271 (9662f157 or later); before that
# "iw phy" offers no wake-on-WLAN and this script says so.
#
# Three patterns, like stock's idea (docs/sleep-stock.md) but for our services:
#   1. MQTT    a TCP packet from the broker (host/port of /var/lib/j49/mqtt.conf) to us
#   2. web     a TCP packet to our port 80
#   3. SSH     a TCP packet to our port 22
# Offsets are in the Ethernet frame: 12-13 type 08 00 (IPv4), 14 = 45 (header of 20 bytes),
# 23 protocol 06 (TCP), 26-29 source address, 30-33 destination address, 34-35 source port,
# 36-37 destination port. "-" is a byte that is not compared.
# the driver is bound a second time at boot (j49-wlan-reprobe), so the name is not always phy0
PHY=$(iw phy | sed -n "s/^Wiphy //p" | head -n 1)
case "$1" in
  show) iw phy $PHY wowlan show; exit ;;
  off)  iw phy $PHY wowlan disable; exit ;;
esac
iw phy | grep -q "WoWLAN support" || { echo "this kernel offers no wake-on-WLAN (iw phy)"; exit 1; }
hexip() { echo "$1" | awk -F. '{printf "%02x:%02x:%02x:%02x", $1, $2, $3, $4}'; }
OWN=$(ip -4 -o addr show wlan0 | awk '{print $4}' | cut -d/ -f1)
[ -n "$OWN" ] || { echo "wlan0 has no address"; exit 1; }
HOST=$(sed -n 's/^host=//p' /var/lib/j49/mqtt.conf 2>/dev/null)
PORT=$(sed -n 's/^port=//p' /var/lib/j49/mqtt.conf 2>/dev/null); PORT=${PORT:-1883}
# The WL1271 only takes patterns that start at byte 0 ("maximum packet offset 0 bytes" in
# iw phy), so the 12 address bytes in front are given as "not compared".
HEAD="-:-:-:-:-:-:-:-:-:-:-:-:08:00:45:-:-:-:-:-:-:-:-:06:-:-"
ME=$(hexip "$OWN")
set --
case "$HOST" in
  [0-9]*.[0-9]*.[0-9]*.[0-9]*)
    set -- "$HEAD:$(hexip "$HOST"):$ME:$(printf '%02x:%02x' $((PORT >> 8)) $((PORT & 255)))" ;;
  *) echo "no broker address in mqtt.conf (or a name, not an address): no MQTT pattern" ;;
esac
set -- "$@" "$HEAD:-:-:-:-:$ME:-:-:00:50" "$HEAD:-:-:-:-:$ME:-:-:00:16"
iw phy $PHY wowlan enable patterns "$@" && iw phy $PHY wowlan show
