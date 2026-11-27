#!/usr/bin/env python3
"""Look at, check or convert a backup file of the J49 installer (.j49backup).

    j49backup.py check  <file.j49backup>                 every checksum; a table per partition
    j49backup.py image  <file.j49backup> <out.bin>       one plain image of the whole flash memory
                                                         (empty and bad blocks as 0xFF)
    j49backup.py split  <file.j49backup> <directory>     one file per partition
    j49backup.py join   <directory> <out.j49backup>      a backup file from one file per partition
                                                         named <anything><name>.bin, e.g. mtd2_spl0.bin

The format is described in tools/web-installer/js/service.js.
"""
import json, os, sys, zlib

LAYOUT = [("ipl", 0x40000), ("spl0", 0x180000), ("spl1", 0x180000), ("env0", 0x60000), ("env1", 0x60000),
          ("boot0", 0x800000), ("root0", 0x2e00000), ("boot1", 0x800000), ("root1", 0x2e00000),
          ("system-config", 0x400000), ("user-config", 0x800000), ("data", 0x1400000), ("log", 0x1400000), ("scratch", 0x5c00000)]
ERASE = 0x20000

def parts(path):
    """Yield (name, size, erasesize, [(kind, data-or-None)]) per partition; checks every sum."""
    with open(path, "rb") as f:
        if f.readline() != b"J49BACKUP 1\n":
            sys.exit("not a J49 backup file")
        info = f.readline()
        if not info.startswith(b"INFO "):
            sys.exit("damaged: no INFO")
        yield json.loads(info[5:])
        while True:
            l = f.readline().decode().split()
            if l == ["END"]:
                return
            if len(l) != 4 or l[0] != "PART":
                sys.exit("damaged: no PART line")
            name, size, erase = l[1], int(l[2]), int(l[3])
            frames, crc = [], 0
            for _ in range(size // erase):
                kind = f.read(1).decode("latin-1")
                if kind in "DX":
                    data = f.read(erase)
                    if len(data) != erase: sys.exit("damaged: file ends in " + name)
                    crc = zlib.crc32(data, crc); frames.append((kind, data))
                elif kind in "EB":
                    frames.append((kind, None))
                else:
                    sys.exit("damaged: frame in " + name)
            if f.readline().decode().strip() != "SUM %08x" % crc:
                sys.exit("damaged: checksum of " + name)
            yield name, size, erase, frames

def check(path):
    it = parts(path); info = next(it)
    print("made:", info.get("date", "?"))
    print("%-14s %9s  %5s %5s %4s %s" % ("partition", "size", "data", "empty", "bad", "read errors"))
    total = errors = 0
    for name, size, erase, frames in it:
        n = {k: sum(1 for kind, _ in frames if kind == k) for k in "DEBX"}
        print("%-14s %9d  %5d %5d %4d %s" % (name, size, n["D"], n["E"], n["B"], n["X"] or ""))
        total += size; errors += n["X"]
    print("whole file: checksums ok, %d MB of flash, %d blocks with read errors" % (total >> 20, errors))
    return errors

def image(path, out):
    it = parts(path); next(it)
    with open(out, "wb") as o:
        for name, size, erase, frames in it:
            for kind, data in frames:
                o.write(data if data else b"\xff" * erase)
    print("wrote", out, os.path.getsize(out), "bytes")

def split(path, d):
    os.makedirs(d, exist_ok=True)
    it = parts(path); next(it)
    for name, size, erase, frames in it:
        with open(os.path.join(d, name + ".bin"), "wb") as o:
            for kind, data in frames:
                o.write(data if data else b"\xff" * erase)
    print("wrote one file per partition in", d)

def join(d, out):
    files = os.listdir(d)
    with open(out, "wb") as o:
        o.write(b"J49BACKUP 1\n")
        o.write(b"INFO " + json.dumps({"date": "joined from " + os.path.basename(os.path.abspath(d)),
                                        "partitions": [{"name": n, "size": s, "erasesize": ERASE} for n, s in LAYOUT]}).encode() + b"\n")
        for name, size in LAYOUT:
            match = [f for f in files if f.endswith(name + ".bin")]
            if len(match) != 1:
                sys.exit("need exactly one file ending in %s.bin, found %d" % (name, len(match)))
            data = open(os.path.join(d, match[0]), "rb").read()
            if len(data) != size:
                sys.exit("%s is %d bytes, the partition is %d" % (match[0], len(data), size))
            o.write(("PART %s %d %d\n" % (name, size, ERASE)).encode())
            crc = 0
            for off in range(0, size, ERASE):
                block = data[off:off + ERASE]
                if block.count(0xff) == ERASE:
                    o.write(b"E")
                else:
                    o.write(b"D" + block); crc = zlib.crc32(block, crc)
            o.write(("SUM %08x\n" % crc).encode())
        o.write(b"END\n")
    print("wrote", out, os.path.getsize(out), "bytes")
    check(out)

a = sys.argv[1:]
if len(a) == 2 and a[0] == "check": sys.exit(1 if check(a[1]) else 0)
elif len(a) == 3 and a[0] == "image": image(a[1], a[2])
elif len(a) == 3 and a[0] == "split": split(a[1], a[2])
elif len(a) == 3 and a[0] == "join": join(a[1], a[2])
else: sys.exit(__doc__)
