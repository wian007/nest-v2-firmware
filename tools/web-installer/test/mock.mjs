// Mocks for the tests: a boot ROM + loader, a DFU device, and the real nandio on pipes.
import { spawn } from "node:child_process";

const ok = data => ({ status: "ok", data: data ? new DataView(data.buffer, data.byteOffset, data.byteLength) : undefined });
const tag = s => new Uint8Array([...s].map(c => c.charCodeAt(0)));
const le = v => { const b = new Uint8Array(4); new DataView(b.buffer).setUint32(0, v >>> 0, true); return b; };
const cat = (...a) => { const b = new Uint8Array(a.reduce((n, x) => n + x.length, 0)); let o = 0; for (const x of a) { b.set(x, o); o += x.length; } return b; };

/** The OMAP boot ROM and the x-loader behind it, as tools/omap-loader talks to them. */
export class MockRom {
  constructor() { this.state = "rom"; this.inq = [Uint8Array.from({ length: 69 }, (_, i) => [1, 5, 1, 5, 0x36, 0x30, 7, 7][i] || 0)]; this.waiters = []; this.buf = new Uint8Array(0); this.loaded = []; this.jump = null; this.configuration = null; this.want = 0; }
  async open() {} async close() {} async claimInterface() {}
  async selectConfiguration() { this.configuration = {}; }
  push(b) { const w = this.waiters.shift(); if (w) w(ok(b)); else this.inq.push(b); }
  async clearHalt() {}
  transferIn() {
    // the loader is not ready at once: its first read fails, as on the real device
    if (this.state === "ask" && !this.failedOnce) { this.failedOnce = true; return Promise.reject(new Error("A transfer error has occurred.")); }
    const b = this.inq.shift(); return b ? Promise.resolve(ok(b)) : new Promise(r => this.waiters.push(r)); }
  async transferOut(ep, data) {
    if (data.length > 512) throw new Error("more than 512 bytes in one transfer");
    this.buf = cat(this.buf, data);
    const u32 = o => new DataView(this.buf.buffer, this.buf.byteOffset).getUint32(o, true);
    for (;;) {
      if (this.state === "rom" && this.buf.length >= 8) {
        if (u32(0) !== 0xf0030002) throw new Error("no peripheral boot command");
        this.want = u32(4); this.buf = this.buf.slice(8); this.state = "romdata";
      } else if (this.state === "romdata" && this.buf.length >= this.want) {
        this.loaded.push({ addr: null, data: this.buf.slice(0, this.want) }); this.buf = this.buf.slice(this.want);
        this.state = "ask"; this.push(cat(tag("USBm"), tag("hello\0"))); this.push(cat(tag("USBf"), le(0)));
      } else if (this.state === "ask" && this.buf.length >= 12) {
        const cmd = String.fromCharCode(...this.buf.slice(0, 4));
        if (cmd === "USBj") { this.jump = u32(4); this.buf = this.buf.slice(12); this.state = "run"; }
        else if (cmd === "USBs") { this.want = u32(4); this.addr = u32(8); this.buf = this.buf.slice(12); this.state = "data"; this.push(cat(tag("USBn"), le(this.want))); }
        else throw new Error("unexpected command " + cmd);
      } else if (this.state === "data" && this.buf.length >= this.want) {
        this.loaded.push({ addr: this.addr, data: this.buf.slice(0, this.want) }); this.buf = this.buf.slice(this.want);
        this.push(cat(tag("USBo"), le(this.want))); this.state = "ask"; this.push(cat(tag("USBf"), le(0)));
      } else break;
    }
    return { status: "ok" };
  }
}

/** A DFU device with named targets, the way U-Boot presents them. */
export class MockDfu {
  constructor(names, transfer = 4096) {
    this.names = names; this.transfer = transfer; this.alt = 0; this.state = 2; this.data = {}; this.cur = []; this.detached = false; this.polls = 0;
    this.configuration = { interfaces: [{ interfaceNumber: 0, alternates: names.map((n, i) => ({ alternateSetting: i, interfaceClass: 0xfe, interfaceSubclass: 1, interfaceName: null })) }] };
  }
  async open() {} async close() {} async claimInterface() {} async selectConfiguration() {}
  async selectAlternateInterface(i, alt) { this.alt = alt; }
  async controlTransferIn(setup, len) {
    if (setup.requestType === "standard") {
      if (setup.value >> 8 === 3) {            // a string descriptor: the name of a target
        const name = this.names[(setup.value & 255) - 4] || "";
        return ok(Uint8Array.from([2 + 2 * name.length, 3, ...[...name].flatMap(c => [c.charCodeAt(0), 0])]));
      }
      // configuration descriptor: one interface descriptor per target (string index 4, 5, ...),
      // then the DFU functional descriptor
      const alts = this.names.flatMap((_, i) => [9, 4, 0, i, 0, 0xfe, 1, 2, 4 + i]);
      return ok(Uint8Array.from([9, 2, 0, 0, 1, 1, 0, 0x80, 50, ...alts, 9, 0x21, 0x0d, 0, 0, this.transfer & 255, this.transfer >> 8, 0x10, 1]));
    }
    if (setup.request !== 3) throw new Error("unexpected request " + setup.request);
    // one busy answer after every block, then the real state
    if (this.state === 3) { this.state = (this.polls++ % 2) ? 5 : 4; if (this.state === 4) return ok(Uint8Array.of(0, 2, 0, 0, 4, 0)); }
    if (this.state === 4) this.state = 5;
    if (this.state === 6) { this.data[this.names[this.alt]] = cat(...this.cur); this.cur = []; this.state = 2; }
    return ok(Uint8Array.of(0, 1, 0, 0, this.state, 0));
  }
  async controlTransferOut(setup, data) {
    if (setup.request === 1) {
      if (data && data.length > this.transfer) throw new Error("block larger than wTransferSize");
      if (!data || data.length === 0) this.state = 6; else { if (setup.value !== this.cur.length % 65536) throw new Error("block number"); this.cur.push(data.slice()); this.state = 3; }
    } else if (setup.request === 0) this.detached = true;
    else if (setup.request === 6 || setup.request === 4) this.state = 2;
    return { status: "ok" };
  }
}

/** nandio itself (the host build) on pipes, with fake partitions in `dir`. */
export function nandio(exe, dir) {
  const p = spawn(exe, [], { env: { NANDIO_FAKE: dir }, stdio: ["pipe", "pipe", "inherit"] });
  const chunks = []; let wake = null, ended = false;
  p.stdout.on("data", c => { chunks.push(new Uint8Array(c)); if (wake) { wake(); wake = null; } });
  p.stdout.on("end", () => { ended = true; if (wake) wake(); });
  const pull = async () => { while (!chunks.length) { if (ended) return null; await new Promise(r => wake = r); } return chunks.shift(); };
  const write = data => new Promise((res, rej) => p.stdin.write(data, e => e ? rej(e) : res()));
  return { pull, write, done: () => new Promise(r => p.on("exit", r)) };
}
