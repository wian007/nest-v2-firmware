#!/usr/bin/env python3
"""Talk to nandio on a serial port from a PC, without the browser: who the board says it is,
its partitions, and how fast a few partitions read. Changes nothing on the thermostat.

    probe.py [/dev/ttyACM0] [partition ...]      (default: ipl spl0 env0 system-config)
"""
import os, select, sys, termios, time

port = sys.argv[1] if len(sys.argv) > 1 else "/dev/ttyACM0"
names = sys.argv[2:] or ["ipl", "spl0", "env0", "system-config"]
fd = os.open(port, os.O_RDWR | os.O_NOCTTY)
a = termios.tcgetattr(fd)
a[0] = a[1] = a[3] = 0; a[2] = termios.CS8 | termios.CREAD | termios.CLOCAL; a[6][termios.VMIN] = 1; a[6][termios.VTIME] = 0
termios.tcsetattr(fd, termios.TCSANOW, a)
buf = b""

def fill(t):
    global buf
    if not select.select([fd], [], [], t)[0]:
        raise SystemExit("no data for %s s" % t)
    buf += os.read(fd, 1 << 20)

def line(t=5):
    global buf
    while b"\n" not in buf: fill(t)
    l, buf = buf.split(b"\n", 1)
    return l.decode("latin-1").rstrip("\r")

def take(n, t=60):
    global buf
    while len(buf) < n: fill(t)
    d, buf = buf[:n], buf[n:]
    return d

def cmd(c):
    os.write(fd, (c + "\n").encode())

cmd("HELLO probe")
while line(8) != "HELLO probe":
    pass
cmd("IDENT"); print(line())
cmd("INFO")
while True:
    l = line()
    if l == "OK": break
    print(" ", l)
for name in names:
    cmd("READ " + name)
    head = line().split()
    if head[0] != "DATA":
        print(name, " ".join(head)); continue
    size, erase = int(head[1]), int(head[2])
    kinds, nbytes, t0 = {}, 0, time.time()
    for _ in range(size // erase):
        k = take(1).decode(); kinds[k] = kinds.get(k, 0) + 1
        if k in "DX": take(erase); nbytes += erase
    line(); ok = line(); dt = time.time() - t0
    print("%-14s %-28s %5.1f s  %s  %s" % (name, kinds, dt, ("%.0f kB/s" % (nbytes / dt / 1000)) if nbytes else "-", ok))
os.close(fd)
