// Tests of the installer's protocol code, without a thermostat: `node test/run.mjs <nandio-host>`
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomBytes } from "node:crypto";
import { romBoot } from "../js/omap.js";
import { Dfu } from "../js/dfu.js";
import { Service, ByteReader, readBackup, verifyBackup, crc32, notAJ49, J49_LAYOUT, NEVER_WRITTEN } from "../js/service.js";
import { MockRom, MockDfu, nandio } from "./mock.mjs";

const rnd = n => new Uint8Array(randomBytes(n));
const same = (a, b) => a.length === b.length && Buffer.from(a).equals(Buffer.from(b));
const ERASE = 0x20000;

// --- crc32 is the usual one
assert.equal(crc32(new TextEncoder().encode("123456789")), 0xcbf43926);

// --- is it a J49: the layout must match exactly, the model must not say otherwise
{
  const good = [{ name: "nand0", size: 0x10000000, erasesize: 0x20000 }, ...J49_LAYOUT.map(([name, size]) => ({ name, size, erasesize: 0x20000 }))];
  assert.equal(notAJ49(good, { model: "Display-2.14" }), null);
  assert.equal(notAJ49(good, { model: "-" }), null);
  assert.match(notAJ49(good, { model: "Display-1.9" }), /Display-1.9/);
  assert.match(notAJ49(good.slice(0, 9), {}), /partitions/);
  const other = good.map(p => ({ ...p })); other[6].size = 0x2000000;
  assert.match(notAJ49(other, {}), /boot0/);
  console.log("ok  device check");
}

// --- the ROM push: three files, the loader's messages, the jump
{
  const rom = new MockRom();
  const files = [{ name: "x-load", data: rnd(27876) }, { name: "u-boot", data: rnd(100000), addr: 0x80100000 }, { name: "kernel", data: rnd(300001), addr: 0x80a00000 }];
  const logs = []; let last = 0;
  await romBoot(rom, files, 0x80100000, { log: l => logs.push(l), progress: (d, t) => { assert(d >= last && d <= t); last = d; } });
  assert.equal(rom.loaded.length, 3);
  files.forEach((f, i) => { assert(same(rom.loaded[i].data, f.data), f.name); if (i) assert.equal(rom.loaded[i].addr, f.addr); });
  assert.equal(rom.jump, 0x80100000);
  assert.equal(last, 27876 + 100000 + 300001);
  assert(logs.some(l => l.includes("loader: hello")));
  // a thermostat that hangs in boot mode: the first read fails
  const dead = new MockRom(); dead.transferIn = async () => { throw new Error("A transfer error has occurred."); };
  await assert.rejects(romBoot(dead, files, 0x80100000), e => e.code === "stale");
  console.log("ok  ROM push");
}

// --- DFU: block size from the descriptor, targets by name, detach
{
  const dev = new MockDfu(["installer", "rootfs", "boot0"], 4096);
  const dfu = new Dfu(dev);
  assert.deepEqual(await dfu.open(), ["installer", "rootfs", "boot0"]);
  assert.equal(dfu.transfer, 4096);
  const kernel = rnd(4096 * 3 + 17), root = rnd(50000);
  await dfu.download("installer", kernel);
  await dfu.download("rootfs", root);
  assert(same(dev.data.installer, kernel) && same(dev.data.rootfs, root));
  await assert.rejects(dfu.download("nope", root), /no target/);
  await dfu.detach();
  assert(dev.detached);
  console.log("ok  DFU download");
}

// --- the service, against the real nandio: backup, new image, backup back
{
  const exe = process.argv[2];
  const dir = mkdtempSync(join(tmpdir(), "j49-inst-"));
  const orig = {
    ipl: Buffer.concat([randomBytes(ERASE), Buffer.alloc(ERASE, 0xff)]),
    spl0: Buffer.concat([randomBytes(2 * ERASE), Buffer.alloc(2 * ERASE, 0xff)]),
    spl1: Buffer.concat([randomBytes(2 * ERASE), Buffer.alloc(2 * ERASE, 0xff)]),
    root0: Buffer.concat([randomBytes(ERASE), Buffer.concat([Buffer.from("BADBLOCK"), Buffer.alloc(ERASE - 8)]), randomBytes(3 * ERASE), Buffer.alloc(3 * ERASE, 0xff)]),
  };
  const total = Object.values(orig).reduce((n, b) => n + b.length, 0);
  let mtd = 'dev:    size   erasesize  name\nmtd0: ' + total.toString(16).padStart(8, "0") + ' 00020000 "nand0"\n';
  Object.entries(orig).forEach(([n, b], i) => { mtd += "mtd" + (i + 1) + ": " + b.length.toString(16).padStart(8, "0") + ' 00020000 "' + n + '"\n'; writeFileSync(join(dir, n), b); });
  writeFileSync(join(dir, "mtd"), mtd); writeFileSync(join(dir, "nand0"), Buffer.alloc(total, 0xff));
  const dev = nandio(exe, dir);
  const svc = new Service(new ByteReader(dev.pull), dev.write);
  await svc.start();
  const parts = await svc.info();
  assert.deepEqual(parts.map(p => p.name), ["nand0", "ipl", "spl0", "spl1", "root0"]);

  // backup into memory
  const pieces = [];
  const summary = await svc.backup(async b => { pieces.push(b.slice()); }, () => {}, { date: "2026-01-01" });
  const file = Buffer.concat(pieces);
  assert.deepEqual(summary.find(s => s.name === "root0"), { name: "root0", D: 4, E: 3, B: 1, X: 0 });
  assert(file.length < total / 2 + 4096, "empty blocks are not stored");
  const fileReader = () => { let sent = false; return new ByteReader(async () => { if (sent) return null; sent = true; return new Uint8Array(file); }); };
  const listed = await verifyBackup(fileReader());
  assert.deepEqual(listed.map(p => p.name), ["ipl", "spl0", "spl1", "root0"]);
  // a damaged file is found
  const broken = Buffer.from(file); broken[file.length - 200000] ^= 1;
  await assert.rejects(verifyBackup(new ByteReader((() => { let s = false; return async () => s ? null : (s = true, new Uint8Array(broken)); })())), /damaged|checksum/);

  // "install": our U-Boot into both spl partitions, something else into root0
  const uboot = rnd(ERASE + 5000);
  await svc.writeImage("spl0", uboot); await svc.writeImage("spl1", uboot);
  await svc.writeImage("root0", rnd(2 * ERASE));
  const now = readFileSync(join(dir, "spl0"));
  assert(same(now.subarray(0, uboot.length), uboot) && now.subarray(uboot.length).every(b => b === 0xff));
  await assert.rejects(svc.writeImage("spl0", rnd(5 * ERASE)), /larger than the partition/);
  await assert.rejects(svc.writeImage("nope", rnd(10)), /no partition/);

  // the backup back: everything is as it was, the loader partition was not written
  writeFileSync(join(dir, "ipl"), Buffer.alloc(2 * ERASE, 0xaa));   // if it were written, this would change
  for await (const p of readBackup(fileReader())) {
    if (!p.name || NEVER_WRITTEN.includes(p.name)) continue;
    await svc.writeFrames(p.name, p.frames, "exact");
  }
  for (const n of ["spl0", "spl1", "root0"]) assert(same(readFileSync(join(dir, n)), orig[n]), n + " is as it was");
  assert(readFileSync(join(dir, "ipl")).every(b => b === 0xaa));
  await svc.reboot();
  assert.equal(await dev.done(), 0);
  console.log("ok  backup, install, restore against nandio");
}
console.log("all installer tests passed");
