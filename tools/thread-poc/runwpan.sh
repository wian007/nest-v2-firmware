#!/bin/sh
# PoC: stock wpantund + spi-server in the stock chroot on our kernel. Never updates NCP firmware.
R=/mnt/scratch/root; G=/sys/class/gpio
mountpoint -q /mnt/scratch || mount -t jffs2 /dev/mtdblock13 /mnt/scratch
for n in 573 694; do [ -d $G/gpio$n ] || echo $n > $G/export; done
echo falling > $G/gpio694/edge
[ "$(cat $G/gpio573/direction)" = out ] || echo high > $G/gpio573/direction
for d in dev proc sys; do mountpoint -q $R/$d || mount --bind /$d $R/$d; done
cat > $R/etc/wpantund.j49.conf <<'C'
WPANInterfaceName wpan0
NCPSocketName "system:/usr/sbin/spi-server -p - -s /dev/spidev1.0 -i /sys/class/gpio/gpio694/value -r /sys/class/gpio/gpio573/value -T 0x00"
EmberUseASH 0
TXPower -15
TXPowerMode 5
NCPHardResetPath "/sys/class/gpio/gpio573/value"
com.nestlabs.internal:Legacy:Enabled true
com.nestlabs.internal:Legacy:PreferInterface true
com.silabs:NCPInitFlushTime 0.25
ActiveWakeupMask 1
NCPTXPowerLimit -10
AutoUpdateFirmware false
C
chroot $R /bin/sh -c 'mkdir -p /var/run/dbus /var/lib/dbus; dbus-uuidgen --ensure; rm -f /var/run/dbus/pid; pidof dbus-daemon >/dev/null || dbus-daemon --system --fork'
sleep 2
chroot $R /bin/sh -c 'LD_LIBRARY_PATH=/nestlabs/lib:/usr/lib:/lib exec /usr/sbin/wpantund -c /etc/wpantund.j49.conf' > /run/wpantund.out 2>&1 &
W=$!
sleep ${1:-25}
echo "=== wpanctl status"; chroot $R /bin/sh -c 'LD_LIBRARY_PATH=/nestlabs/lib:/usr/lib:/lib wpanctl status' 2>&1 | cut -c1-120
echo "=== wpanctl getprop"; chroot $R /bin/sh -c 'LD_LIBRARY_PATH=/nestlabs/lib:/usr/lib:/lib wpanctl getprop' 2>&1 | cut -c1-140 | sed -n "1,60p"
echo "=== ifaces"; ls /sys/class/net | tr "\n" " "; echo; ip addr show wpan0 2>&1 | cut -c1-110
echo "=== wpantund out"; tail -n 25 /run/wpantund.out | cut -c1-170
kill $W 2>/dev/null; sleep 1; chroot $R /bin/sh -c 'killall wpantund spi-server dbus-daemon' 2>/dev/null
for d in dev proc sys; do umount $R/$d 2>/dev/null; done; umount /mnt/scratch 2>/dev/null
echo DONE
