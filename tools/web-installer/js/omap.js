// The first step of the install: start the thermostat over USB from the boot ROM of its
// OMAP3 (USB id 0451:d00e, there for a few seconds after power-on with the ring held).
// The same protocol as tools/omap-loader (omap_loader.c): the ROM takes a small second-stage
// loader (x-loader) into its internal RAM and runs it; that loader then asks for more files,
// each with a load address, and jumps to one of them.
//
// `dev` is a WebUSB USBDevice (or anything with the same methods: tests use a mock).

export const ROM_FILTER = { vendorId: 0x0451, productId: 0xd00e };
const PERIPH_BOOT = 0xf0030002;
const ASIC_ID_LEN = 69;
const CHUNK = 512; // the size omap_loader sends at a time; larger was less reliable there
const tag = s => (s.charCodeAt(0) | (s.charCodeAt(1) << 8) | (s.charCodeAt(2) << 16) | (s.charCodeAt(3) << 24)) >>> 0;
const CMD = { FILE: tag("USBs"), FILE_REQ: tag("USBf"), JUMP: tag("USBj"), ECHO_SZ: tag("USBn"), REPORT_SZ: tag("USBo"), MESSAGE: tag("USBm") };

const le32 = (...v) => { const b = new Uint8Array(4 * v.length), d = new DataView(b.buffer); v.forEach((x, i) => d.setUint32(4 * i, x >>> 0, true)); return b; };
const sleep = ms => new Promise(r => setTimeout(r, ms));

/** Reads with a time limit. A WebUSB read cannot be cancelled, so one that ran out of time
 *  stays pending and the next call waits for that same read. */
function reader(dev, ep) {
  let pending = null;
  return async (length, ms) => {
    if (!pending) pending = dev.transferIn(ep, length);
    const timeout = Symbol("timeout");
    const r = await Promise.race([pending, sleep(ms).then(() => timeout)]);
    if (r === timeout) return null;
    pending = null;
    if (r.status !== "ok") throw new Error("USB read: " + r.status);
    return new Uint8Array(r.data.buffer, r.data.byteOffset, r.data.byteLength);
  };
}
async function send(dev, ep, data, onBytes) {
  for (let i = 0; i < data.length; i += CHUNK) {
    const part = data.subarray(i, Math.min(i + CHUNK, data.length));
    const r = await dev.transferOut(ep, part);
    if (r.status !== "ok") throw new Error("USB write: " + r.status);
    if (onBytes) onBytes(part.length);
  }
}

/**
 * files: [{name, data: Uint8Array, addr}] - the first is the x-loader for the ROM (no address),
 * the others go to their addresses. jump: the address to start. progress(done, total, name).
 */
export async function romBoot(dev, files, jump, { log = () => {}, progress = () => {} } = {}) {
  if (files.length < 2) throw new Error("need the loader and at least one file");
  await dev.open();
  if (!dev.configuration) await dev.selectConfiguration(1);
  await dev.claimInterface(0);
  const read = reader(dev, 1);
  const total = files.reduce((n, f) => n + f.data.length, 0);
  let done = 0;
  const count = name => n => { done += n; progress(done, total, name); };

  const id = await read(512, 5000);
  if (!id) throw new Error("the boot ROM did not answer");
  if (id.length !== ASIC_ID_LEN) throw new Error("unexpected answer from the boot ROM (" + id.length + " bytes)");
  log("boot ROM: " + Array.from(id.subarray(1, 8), b => b.toString(16).padStart(2, "0")).join(""));

  const first = files[0];
  await send(dev, 1, le32(PERIPH_BOOT));
  await send(dev, 1, le32(first.data.length));
  await send(dev, 1, first.data, count(first.name));
  log(first.name + " sent to the ROM");

  // the loader now runs and asks for the other files, one at a time
  let cur = 1, misses = 0;
  while (cur < files.length) {
    const f = files[cur];
    const msg = await read(512, 3000);
    if (!msg) {
      if (++misses >= 5) throw new Error("the loader does not answer");
      continue;
    }
    misses = 0;
    if (msg.length < 8) throw new Error("short answer from the loader");
    const d = new DataView(msg.buffer, msg.byteOffset, msg.byteLength);
    const op = d.getUint32(0, true), arg = d.getUint32(4, true);
    if (op === CMD.FILE_REQ) {
      await send(dev, 1, le32(CMD.FILE, f.data.length, f.addr));
    } else if (op === CMD.ECHO_SZ) {
      if (arg !== f.data.length) throw new Error("the loader expects " + arg + " bytes for " + f.name);
      await send(dev, 1, f.data, count(f.name));
    } else if (op === CMD.REPORT_SZ) {
      if (arg !== f.data.length) throw new Error("the loader received " + arg + " of " + f.data.length + " bytes of " + f.name);
      log(f.name + " loaded at 0x" + f.addr.toString(16));
      cur++;
    } else if (op === CMD.MESSAGE) {
      let s = ""; for (let i = 4; i < msg.length && msg[i]; i++) s += String.fromCharCode(msg[i]);
      log("loader: " + s.trim());
    } else {
      throw new Error("unknown answer from the loader: 0x" + op.toString(16));
    }
  }
  await send(dev, 1, le32(CMD.JUMP, jump, 0));
  log("started at 0x" + jump.toString(16));
  try { await dev.close(); } catch (e) { /* the device is gone: it runs the new code */ }
}
