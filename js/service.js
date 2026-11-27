// The service on the thermostat: `nandio` (tools/dfu-install/nandio/nandio.c) in the rescue
// kernel, on the USB serial port. With it the installer makes a backup, writes our U-Boot and
// puts a backup back. The protocol is described at the top of nandio.c.
//
// A backup file is the same stream nandio sends, with a text line before each partition:
//   J49BACKUP 1
//   INFO <json: date, partitions>
//   PART <name> <size> <erasesize>
//     one frame per erase block: 'D' + data | 'E' (empty) | 'B' (bad) | 'X' + data (read error)
//   SUM <crc32 hex of the data in D and X frames>
//   ...
//   END

const enc = new TextEncoder(), dec = new TextDecoder();
let table = null;
export function crc32(data, crc = 0) {
  if (!table) {
    table = new Uint32Array(256);
    for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; table[n] = c >>> 0; }
  }
  crc = ~crc >>> 0;
  for (let i = 0; i < data.length; i++) crc = table[(crc ^ data[i]) & 0xff] ^ (crc >>> 8);
  return ~crc >>> 0;
}
const hex8 = n => (n >>> 0).toString(16).padStart(8, "0");

/** Bytes that come in pieces, read back as exact counts and lines. `pull()` gives the next
 *  piece (Uint8Array) or null at the end. */
export class ByteReader {
  constructor(pull) { this.pull = pull; this.buf = new Uint8Array(0); this.pos = 0; }
  async fill() {
    const more = await this.pull();
    if (!more) throw new Error("the connection ended");
    const rest = this.buf.subarray(this.pos);
    const b = new Uint8Array(rest.length + more.length); b.set(rest); b.set(more, rest.length);
    this.buf = b; this.pos = 0;
  }
  async bytes(n) {
    while (this.buf.length - this.pos < n) await this.fill();
    const out = this.buf.slice(this.pos, this.pos + n); this.pos += n; return out;
  }
  async byte() { return (await this.bytes(1))[0]; }
  async line() {
    for (;;) {
      const i = this.buf.indexOf(10, this.pos);
      if (i >= 0) { const s = dec.decode(this.buf.subarray(this.pos, i)).replace(/\r$/, ""); this.pos = i + 1; return s; }
      if (this.buf.length - this.pos > 4096) throw new Error("no line end where one was expected");
      await this.fill();
    }
  }
}

/** The flash layout of a Nest Learning Thermostat of the 2nd generation (board J49), as the
 *  kernel of the service reports it (docs/flash-layout.md): name and size of every partition. */
export const J49_LAYOUT = [
  ["ipl", 0x40000], ["spl0", 0x180000], ["spl1", 0x180000], ["env0", 0x60000], ["env1", 0x60000],
  ["boot0", 0x800000], ["root0", 0x2e00000], ["boot1", 0x800000], ["root1", 0x2e00000],
  ["system-config", 0x400000], ["user-config", 0x800000], ["data", 0x1400000], ["log", 0x1400000], ["scratch", 0x5c00000],
];
/** Is this the thermostat the installer is for? Returns null when it is, else why not.
 *  Two things must hold: the flash layout is exactly that of the J49, and the model the boot
 *  loader passes, when there is one, is a "Display-2.x". */
export function notAJ49(parts, ident) {
  const mine = parts.filter(p => p.name !== "nand0");
  if (mine.length !== J49_LAYOUT.length) return "it has " + mine.length + " partitions, a J49 has " + J49_LAYOUT.length;
  for (let i = 0; i < J49_LAYOUT.length; i++) {
    const [name, size] = J49_LAYOUT[i], p = mine[i];
    if (p.name !== name || p.size !== size || p.erasesize !== 0x20000) return "partition " + (i + 1) + " is " + p.name + " (" + p.size + " bytes), a J49 has " + name + " (" + size + " bytes) there";
  }
  const model = ident && ident.model;
  if (model && model !== "-" && !/^Display-2\./.test(model)) return "the boot loader calls it " + model + ", not Display-2.x";
  return null;
}

/** The partitions that are not written when a backup goes back: the first-stage loader that
 *  the boot ROM reads (nothing we do changes it) and the alias of the whole chip. */
export const NEVER_WRITTEN = ["ipl", "nand0"];

export class Service {
  /** reader: a ByteReader on the port; write(Uint8Array): send bytes to the port. */
  constructor(reader, write, log = () => {}) { this.r = reader; this.write = write; this.log = log; }
  send(text) { return this.write(enc.encode(text + "\n")); }
  /** A line that is not the start-up banner; "ERR ..." becomes an exception. */
  async answer() {
    for (;;) {
      const l = await this.r.line();
      // the first lines go to the log: they show what the other side is doing when it is stuck
      if ((this.traced = (this.traced || 0) + 1) <= 30) this.log("< " + l.slice(0, 100));
      if (l.startsWith("J49NANDIO") || l.startsWith("HELLO ") || l === "") continue;
      if (l.startsWith("ERR")) throw new Error("the thermostat reports: " + l.slice(4));
      return l;
    }
  }
  /** Get in step with the service: say HELLO with a word of our own, once a second, until
   *  that word comes back; everything before it is thrown away (kernel messages, answers to
   *  what a modem program on the PC typed at the port, an echo). */
  async start() {
    const word = "w" + Math.random().toString(36).slice(2, 10);
    const hello = () => this.send("HELLO " + word).catch(() => {});
    await hello();
    const again = setInterval(hello, 1000);
    try {
      for (;;) {
        const l = await this.r.line();
        if ((this.traced = (this.traced || 0) + 1) <= 30) this.log("< " + l.slice(0, 100));
        if (l === "HELLO " + word) return;
      }
    } finally { clearInterval(again); }
  }
  /** What the panel of the thermostat shows: a title, a line of text and a ring that the next
   *  read, write or erase moves from `base` to `base + span` percent. The panel has capitals,
   *  digits and a few signs only. */
  async show(title, base = 0, span = 0, text = "") {
    const word = s => String(s).toUpperCase().replace(/[^A-Z0-9%:/.,=+<>-]+/g, "_").replace(/^_+|_+$/g, "").slice(0, 22) || "-";
    const b = Math.max(0, Math.min(100, Math.floor(base))), sp = Math.max(0, Math.min(100 - b, Math.floor(span)));
    await this.send("SHOW " + word(title) + " " + b + " " + sp + (text ? " " + word(text) : ""));
    if ((await this.answer()) !== "OK") throw new Error("no OK after SHOW");
  }
  /** What the board says it is: {model, hardware, revision}. */
  async ident() {
    await this.send("IDENT");
    const l = await this.answer();
    const out = {};
    for (const part of l.split(" ").slice(1)) { const i = part.indexOf("="); if (i > 0) out[part.slice(0, i)] = part.slice(i + 1).replace(/_/g, " "); }
    return out;
  }
  /** Ask what partitions there are: [{name, size, erasesize}]. */
  async info() {
    await this.send("INFO");
    const parts = [];
    for (;;) {
      const l = await this.answer();
      if (l === "OK") break;
      const f = l.split(" ");
      if (f[0] === "MTD" && f.length === 4) parts.push({ name: f[1], size: +f[2], erasesize: +f[3] });
    }
    if (!parts.length) throw new Error("the thermostat reports no partitions");
    this.parts = parts;
    return parts;
  }
  /** Read one partition. frame(kind, data|null) is called per erase block. Returns counts. */
  async read(name, frame) {
    await this.send("READ " + name);
    const f = (await this.answer()).split(" ");
    if (f[0] !== "DATA") throw new Error("unexpected answer: " + f.join(" "));
    const size = +f[1], erase = +f[2];
    let crc = 0; const n = { D: 0, E: 0, B: 0, X: 0 };
    for (let off = 0; off < size; off += erase) {
      const kind = String.fromCharCode(await this.r.byte());
      if (!(kind in n)) throw new Error("unexpected data from the thermostat");
      n[kind]++;
      let data = null;
      if (kind === "D" || kind === "X") { data = await this.r.bytes(erase); crc = crc32(data, crc); }
      await frame(kind, data);
    }
    const sum = await this.answer();
    if (sum !== "SUM " + hex8(crc)) throw new Error("checksum of " + name + " does not match: the transfer was damaged");
    if ((await this.answer()) !== "OK") throw new Error("no OK after " + name);
    return { size, erasesize: erase, crc, blocks: n };
  }
  /** Write frames ([data|null, ...], one per erase block) to a partition. mode: "shift" for a
   *  new image, "exact" for a backup of this device. progress(done, total). */
  async writeFrames(name, frames, mode, progress = () => {}) {
    const part = (this.parts || []).find(p => p.name === name);
    if (!part) throw new Error("the thermostat has no partition " + name);
    if (frames.length * part.erasesize > part.size) throw new Error(name + ": the image is larger than the partition");
    await this.send("WRITE " + name + " " + frames.length * part.erasesize + " " + mode);
    const ready = await this.answer();
    if (!ready.startsWith("READY")) throw new Error("unexpected answer: " + ready);
    let crc = 0, skipped = 0;
    for (let i = 0; i < frames.length; i++) {
      const data = frames[i];
      if (data) {
        const m = new Uint8Array(1 + data.length); m[0] = 68; m.set(data, 1); // 'D'
        await this.write(m);
      } else {
        await this.write(Uint8Array.of(69)); // 'E'
      }
      const ans = await this.r.byte();
      if (ans === 98) skipped++;                    // 'b': bad block, left out
      else if (ans !== 46) throw new Error("unexpected answer while writing " + name);
      else if (data) crc = crc32(data, crc);
      progress(i + 1, frames.length);
    }
    const sum = await this.answer();
    if (sum !== "SUM " + hex8(crc)) throw new Error(name + ": what was read back differs from what was sent");
    if ((await this.answer()) !== "OK") throw new Error("no OK after writing " + name);
    return { skipped };
  }
  /** Write an image (Uint8Array) as a new image: padded with 0xFF to whole erase blocks. */
  async writeImage(name, image, progress) {
    const part = (this.parts || []).find(p => p.name === name);
    if (!part) throw new Error("the thermostat has no partition " + name);
    const frames = [];
    for (let off = 0; off < image.length; off += part.erasesize) {
      const b = new Uint8Array(part.erasesize).fill(0xff);
      b.set(image.subarray(off, Math.min(off + part.erasesize, image.length)));
      frames.push(b);
    }
    return this.writeFrames(name, frames, "shift", progress);
  }
  async reboot() { await this.send("REBOOT"); try { await this.answer(); } catch (e) { /* it may be gone */ } }

  /** A full backup: every partition except the alias of the whole chip, written with sink(bytes).
   *  progress(doneBytes, totalBytes, name). Returns a summary per partition. */
  async backup(sink, progress = () => {}, meta = {}, title = "BACKUP") {
    const parts = (this.parts || await this.info()).filter(p => p.name !== "nand0");
    const total = parts.reduce((n, p) => n + p.size, 0);
    let done = 0; const summary = [];
    await sink(enc.encode("J49BACKUP 1\nINFO " + JSON.stringify({ ...meta, partitions: parts }) + "\n"));
    for (const p of parts) {
      await this.show(title, done * 100 / total, (done + p.size) * 100 / total - Math.floor(done * 100 / total), p.name);
      await sink(enc.encode("PART " + p.name + " " + p.size + " " + p.erasesize + "\n"));
      const r = await this.read(p.name, async (kind, data) => {
        await sink(Uint8Array.of(kind.charCodeAt(0)));
        if (data) await sink(data);
        done += p.erasesize; progress(done, total, p.name);
      });
      await sink(enc.encode("SUM " + hex8(r.crc) + "\n"));
      summary.push({ name: p.name, ...r.blocks });
      this.log(p.name + ": " + r.blocks.D + " blocks with data, " + r.blocks.E + " empty" + (r.blocks.B ? ", " + r.blocks.B + " bad" : "") + (r.blocks.X ? ", " + r.blocks.X + " WITH READ ERRORS" : ""));
    }
    await sink(enc.encode("END\n"));
    return summary;
  }
}

/** Read a backup file through a ByteReader. Yields {name, size, erasesize, frames} per
 *  partition, the checksum checked; frames is [data|null] ready for writeFrames("exact"). */
export async function* readBackup(reader) {
  if ((await reader.line()) !== "J49BACKUP 1") throw new Error("this is not a J49 backup file");
  const info = await reader.line();
  if (!info.startsWith("INFO ")) throw new Error("damaged backup file (no INFO)");
  yield { info: JSON.parse(info.slice(5)) };
  for (;;) {
    const l = await reader.line();
    if (l === "END") return;
    const f = l.split(" ");
    if (f[0] !== "PART" || f.length !== 4) throw new Error("damaged backup file (no PART)");
    const size = +f[2], erasesize = +f[3];
    if (!(erasesize > 0) || size % erasesize) throw new Error("damaged backup file (sizes)");
    const frames = []; let crc = 0, errors = 0;
    for (let off = 0; off < size; off += erasesize) {
      const kind = String.fromCharCode(await reader.byte());
      if (kind === "D" || kind === "X") { const d = await reader.bytes(erasesize); crc = crc32(d, crc); frames.push(d); if (kind === "X") errors++; }
      else if (kind === "E" || kind === "B") frames.push(null);
      else throw new Error("damaged backup file (frame)");
    }
    if ((await reader.line()) !== "SUM " + hex8(crc)) throw new Error("damaged backup file: checksum of " + f[1]);
    yield { name: f[1], size, erasesize, frames, errors };
  }
}

/** Check a backup file without a device: every checksum, and that it ends. Returns the list. */
export async function verifyBackup(reader) {
  const out = [];
  for await (const p of readBackup(reader)) if (p.name) out.push({ name: p.name, size: p.size, data: p.frames.filter(Boolean).length, errors: p.errors });
  return out;
}
