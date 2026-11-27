#!/usr/bin/env python3
"""Test nandio on a PC: fake partitions in a directory, the serial line on pipes."""
import os, subprocess, sys, tempfile, zlib

ERASE = 0x20000
here = os.path.dirname(os.path.abspath(__file__))
exe = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "nandio-host")

def start(d):
    return subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env={"NANDIO_FAKE": d})

def line(p):
    return p.stdout.readline().decode().rstrip("\n")

def cmd(p, text):
    p.stdin.write(text.encode() + b"\n"); p.stdin.flush()

def read_part(p, name):
    cmd(p, "READ " + name)
    head = line(p).split()
    assert head[0] == "DATA", head
    size, erase = int(head[1]), int(head[2])
    blocks, crc = [], 0
    for _ in range(size // erase):
        kind = p.stdout.read(1).decode()
        if kind in "DX":
            data = p.stdout.read(erase); crc = zlib.crc32(data, crc)
            blocks.append((kind, data))
        else:
            blocks.append((kind, None))
    assert line(p) == "SUM %08x" % crc
    assert line(p) == "OK"
    return blocks

def write_part(p, name, frames, mode):
    cmd(p, "WRITE %s %d %s" % (name, len(frames) * ERASE, mode))
    ans = line(p)
    if not ans.startswith("READY"):
        return ans
    chunk = int(ans.split()[2])
    crc = 0
    for data in frames:
        if data is None:
            p.stdin.write(b"E"); p.stdin.flush()
        else:
            p.stdin.write(b"D"); p.stdin.flush()
            for at in range(0, len(data), chunk):      # in pieces, each answered with "+"
                p.stdin.write(data[at:at + chunk]); p.stdin.flush()
                assert p.stdout.read(1) == b"+"
        ans = p.stdout.read(1)
        assert ans in (b".", b"b"), ans
        if data is not None and ans == b".":
            crc = zlib.crc32(data, crc)
    ans = line(p)
    if ans.startswith("ERR"):
        return ans
    assert ans == "SUM %08x" % crc, ans
    return line(p)

with tempfile.TemporaryDirectory() as d:
    rnd = lambda n: os.urandom(n)
    spl = rnd(ERASE) + b"\xff" * ERASE + rnd(ERASE) + b"\xff" * ERASE          # 4 blocks, 2 empty
    cfg = rnd(ERASE) + b"BADBLOCK" + b"\0" * (ERASE - 8) + rnd(ERASE)           # block 1 is bad
    open(d + "/mtd", "w").write('dev:    size   erasesize  name\nmtd0: %08x 00020000 "nand0"\nmtd2: %08x 00020000 "spl0"\nmtd9: %08x 00020000 "system-config"\n' % (7 * ERASE, 4 * ERASE, 3 * ERASE))
    open(d + "/nand0", "wb").write(b"\xff" * 7 * ERASE)
    open(d + "/spl0", "wb").write(spl)
    open(d + "/system-config", "wb").write(cfg)
    p = start(d)
    assert line(p) == "J49NANDIO 1"           # the banner, once a second until a command comes
    cmd(p, "INFO")
    info = []
    while True:
        l = line(p)
        if l == "OK": break
        if l.startswith("MTD"): info.append(l)
    assert info == ["MTD nand0 %d %d" % (7 * ERASE, ERASE), "MTD spl0 %d %d" % (4 * ERASE, ERASE), "MTD system-config %d %d" % (3 * ERASE, ERASE)], info

    b = read_part(p, "spl0")
    assert [k for k, _ in b] == ["D", "E", "D", "E"] and b[0][1] == spl[:ERASE] and b[2][1] == spl[2 * ERASE:3 * ERASE]
    b = read_part(p, "system-config")
    assert [k for k, _ in b] == ["D", "B", "D"]

    # a new image, "shift": 2 blocks into spl0; the rest of the partition is erased
    img = [rnd(ERASE), rnd(ERASE)]
    assert write_part(p, "spl0", img, "shift") == "OK"
    got = open(d + "/spl0", "rb").read()
    assert got == img[0] + img[1] + b"\xff" * 2 * ERASE

    # "shift" over a bad block: the data moves up
    assert write_part(p, "system-config", img, "shift") == "OK"
    got = open(d + "/system-config", "rb").read()
    assert got[:ERASE] == img[0] and got[ERASE:ERASE + 8] == b"BADBLOCK" and got[2 * ERASE:] == img[1]
    # too much for what is left
    assert write_part(p, "system-config", [rnd(ERASE)] * 3, "shift").startswith("ERR no room")

    # a backup back, "exact": the bad block is left out, an empty frame erases
    back = [rnd(ERASE), rnd(ERASE), None]
    assert write_part(p, "system-config", back, "exact") == "OK"
    got = open(d + "/system-config", "rb").read()
    assert got[:ERASE] == back[0] and got[ERASE:ERASE + 8] == b"BADBLOCK" and got[2 * ERASE:] == b"\xff" * ERASE

    # refusals
    for bad in ["WRITE spl0 100 shift", "WRITE nope %d shift" % ERASE, "WRITE spl0 %d sideways" % ERASE, "WRITE spl0 %d shift" % (5 * ERASE), "READ nope", "FLY"]:
        cmd(p, bad); assert line(p).startswith("ERR"), bad
    cmd(p, "ERASE spl0"); assert line(p) == "OK"
    assert open(d + "/spl0", "rb").read() == b"\xff" * 4 * ERASE
    cmd(p, "REBOOT"); assert line(p) == "OK"
    assert p.wait(5) == 0
print("nandio: all tests passed")
