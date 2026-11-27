#!/bin/sh
R=/mnt/scratch/root
C=/mnt/scratch/cap; mkdir -p $C
( while :; do echo "alive $(date +%T) up=$(cut -d" " -f1 /proc/uptime) nlclient=$(pidof nlclient)" >> $C/alive.log; dmesg | tail -n 5 > $C/dmesg.last; sync; sleep 4; done ) &
ALIVE=$!
systemctl stop bpd 2>/dev/null
# bind kernel fs
mkdir -p $R/media/system-config $R/media/user-config; mountpoint -q $R/media/system-config || mount -t jffs2 -o ro /dev/mtdblock9 $R/media/system-config 2>/dev/null; mountpoint -q $R/media/user-config || mount -t jffs2 -o ro /dev/mtdblock10 $R/media/user-config 2>/dev/null
for d in dev proc sys; do mkdir -p $R/$d; mountpoint -q $R/$d || mount --bind /$d $R/$d; done
# nlclient must not arm the hardware watchdog or restart the device
mount --make-private $R/dev 2>/dev/null && for w in $R/dev/watchdog*; do [ -e "$w" ] && mount --bind /dev/null "$w"; done
echo "watchdog nodes: $(ls /dev/watchdog* 2>/dev/null | tr '\n' ' ') hidden: $(grep -c "$R/dev/watchdog" /proc/mounts)"
for b in reboot poweroff halt; do printf '#!/bin/sh\necho "$(date) %s $*" >> /nestlabs/var/log/REBOOT_CMD; sync\n' $b > $R/sbin/$b.fake; chmod 755 $R/sbin/$b.fake; [ -e $R/sbin/$b.stock ] || mv $R/sbin/$b $R/sbin/$b.stock 2>/dev/null; mv -f $R/sbin/$b.fake $R/sbin/$b; done
# MITM: ttyproxy on the real backplate UART, expose its PTY as /dev/ttyO2 in the chroot
ln -sf /dev/ttyS2 /dev/ttyO2.real 2>/dev/null; rm -f /dev/ttyO2.real; MM=$(cat /sys/class/tty/ttyS2/dev); mknod /dev/ttyO2.real c ${MM%:*} ${MM#*:} 2>/dev/null
: > $C/nl_intercept.log
/mnt/scratch/ttyproxy -r /dev/ttyO2.real -b 115200 -L $C/nl_intercept.log >/run/proxy.out 2>&1 &
PROXY=$!
sleep 1
SLAVE=$(sed -n 's/^SLAVE=//p' /run/proxy.out)
echo "proxy slave=$SLAVE"
touch $R/dev/ttyO2; mount --bind "$SLAVE" $R/dev/ttyO2
# run nlclient, capture its log + watch RAM
# adapt the chroot copy of product.config to our kernel
PC=$R/nestlabs/etc/product.config
[ -f $PC.orig ] || cp $PC $PC.orig
sed -e 's#/sys/class/backlight/3-0036#/sys/class/backlight/lm3530#' \
    -e 's#key="buttondevice" value="/dev/input/event2"#key="buttondevice" value="/dev/input/event0"#' \
    -e 's#/sys/devices/platform/omap/omap_i2c.3/i2c-3/3-0036#/sys/class/backlight/lm3530/device#' \
    -e 's#/sys/devices/platform/diamond-backplate/#/nestlabs/var/fakebp/#' $PC.orig > $PC
# fake diamond-backplate sysfs dir: nlclient lstat()s "detect#" and "reset" and wants SYMLINKS to
# sysfs GPIO dirs (stock: gpio65 = Backplate Detect, gpio104 = Backplate Reset; +512 on 6.6).
G=/sys/class/gpio
for n in 577 616; do [ -d $G/gpio$n ] || echo $n > $G/export; done
echo in > $G/gpio577/direction; echo both > $G/gpio577/edge 2>/dev/null
# reset line: become an output WITHOUT changing the level it has now
[ "$(cat $G/gpio616/direction)" = out ] || { [ "$(cat $G/gpio616/value)" = 1 ] && echo high > $G/gpio616/direction || echo low > $G/gpio616/direction; }
F=$R/nestlabs/var/fakebp; mkdir -p $F
umount "$F/detect#" 2>/dev/null; rm -f "$F/detect#" "$F/reset"
ln -s $G/gpio577 "$F/detect#"; ln -s $G/gpio616 "$F/reset"
echo "detect(gpio65)=$(cat $G/gpio577/value) reset(gpio104)=$(cat $G/gpio616/value) dir=$(cat $G/gpio616/direction)"; ls -l $F | cut -c40-120
# start the D-Bus system daemon in the chroot (nlclient connects to the system bus)
chroot $R /bin/sh -c 'mkdir -p /var/run/dbus /var/lib/dbus; dbus-uuidgen --ensure; rm -f /var/run/dbus/pid; dbus-daemon --system --fork'
sleep 2
echo "dbus socket: $(ls /mnt/scratch/root/var/run/dbus/system_bus_socket 2>&1)"
# connman on the chroot bus (nlclient needs net.connman); it must NOT touch our wlan0
chroot $R /bin/sh -c 'mkdir -p /var/lib/connman /var/run/connman; LD_LIBRARY_PATH=/nestlabs/lib:/usr/lib:/lib /usr/sbin/connmand -I wlan0 -P wifi'
sleep 3
echo "connmand: $(pidof connmand)"
echo "=== nlclient start ==="
chroot $R /bin/sh -c 'LD_LIBRARY_PATH=/nestlabs/lib:/usr/lib:/lib exec /strace -f -o /nestlabs/var/log/strace.out -s 64 -e trace=%file,epoll_ctl,inotify_add_watch,ioctl -e signal=none -E LD_PRELOAD=/nestlabs/lib/fbshim.so /nestlabs/sbin/nlclient -config /nestlabs/etc/client.config -config /nestlabs/etc/product.config' >$C/nlc.out 2>&1 &
NLC=$!
for s in $(seq 1 110); do sleep 5; pidof nlclient >/dev/null || break; done
# stop
kill $NLC 2>/dev/null; chroot $R /bin/sh -c 'killall nlclient' 2>/dev/null; sleep 1
umount $R/dev/ttyO2 2>/dev/null; kill $PROXY 2>/dev/null
kill $(pidof connmand) 2>/dev/null; kill $ALIVE 2>/dev/null
echo "=== reboot markers ==="; cat $R/nestlabs/var/log/REBOOT_CMD $R/nestlabs/var/log/REBOOT_SYSCALL 2>&1 | tail -n 6
echo "=== nlclient log: the last lines of this run ==="; tail -n 60 $R/nestlabs/var/log/nlclient | grep -aiE "backplate|failed|MAIN:|error|display|gfx" | cut -c60-230 | tail -n 40
umount "$R/nestlabs/var/fakebp/detect#" 2>/dev/null
echo "=== strace fakebp/ttyO2 ==="; wc -l < $R/nestlabs/var/log/strace.out; grep -an "fakebp\|ttyO2\|gpio" $R/nestlabs/var/log/strace.out | sed -n "1,40p" | cut -c1-220
echo "=== nlclient log: backplate-regels ==="; tail -n 600 $R/nestlabs/var/log/nlclient | grep -ai "backplate" | cut -c12-26,60-220 | tail -n 40
echo "=== intercept: HU->BP (NLCLIENT->REAL) ==="; grep -a "NLCLIENT->REAL" $C/nl_intercept.log | sed -n "1,60p"
echo "=== intercept: BP->HU eerste 25 ==="; grep -a "REAL->NLCLIENT" $C/nl_intercept.log | sed -n "1,25p" | cut -c1-200
echo "=== intercept: BP temp(0x02)/pir(0x07) frames? ==="; grep -ac REAL $C/nl_intercept.log
echo "=== intercept bytes ==="; wc -c $C/nl_intercept.log
sync
systemctl start bpd j49-screen 2>/dev/null
