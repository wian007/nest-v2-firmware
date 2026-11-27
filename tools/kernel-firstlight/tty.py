#!/usr/bin/env python3
"""tty.py DEV SECONDS [cmd...]: raw-open a serial tty, send each cmd (newline-terminated), print all output for SECONDS."""
import os, sys, termios, time, select
dev, secs = sys.argv[1], float(sys.argv[2])
fd = os.open(dev, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
a = termios.tcgetattr(fd)
a[0] = 0; a[1] = 0; a[2] = termios.CS8 | termios.CREAD | termios.CLOCAL; a[3] = 0
a[6][termios.VMIN] = 0; a[6][termios.VTIME] = 0
termios.tcsetattr(fd, termios.TCSANOW, a)
for c in sys.argv[3:]:
    os.write(fd, (c + "\n").encode()); time.sleep(0.3)
end = time.time() + secs
buf = b""
while time.time() < end:
    r, _, _ = select.select([fd], [], [], 0.2)
    if r:
        try:
            d = os.read(fd, 65536)
        except BlockingIOError:
            continue
        if not d: break
        buf += d
sys.stdout.write(buf.decode(errors="replace")); sys.stdout.flush()
