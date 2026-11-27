// USB DFU 1.1 download, as much of it as our U-Boot needs (USB id 0451:df10). U-Boot shows
// three "alternate settings", each a target: "installer" and "rootfs" are windows in RAM,
// "boot0" is the kernel partition in flash (tools/dfu-install/README.md).
//
// `dev` is a WebUSB USBDevice or a mock with the same methods.

export const DFU_FILTER = { vendorId: 0x0451, productId: 0xdf10 };
const REQ = { DETACH: 0, DNLOAD: 1, GETSTATUS: 3, CLRSTATUS: 4, ABORT: 6 };
const STATE = { IDLE: 2, DNLOAD_SYNC: 3, DNBUSY: 4, DNLOAD_IDLE: 5, MANIFEST_SYNC: 6, MANIFEST: 7, MANIFEST_WAIT: 8, ERROR: 10 };
const sleep = ms => new Promise(r => setTimeout(r, ms));

export class Dfu {
  constructor(dev, log = () => {}) { this.dev = dev; this.log = log; this.iface = 0; this.transfer = 4096; }

  async open() {
    const dev = this.dev;
    await dev.open();
    if (!dev.configuration) await dev.selectConfiguration(1);
    const itf = dev.configuration.interfaces.find(i => i.alternates.some(a => a.interfaceClass === 0xfe && a.interfaceSubclass === 1));
    if (!itf) throw new Error("this device has no DFU interface");
    this.iface = itf.interfaceNumber;
    this.alts = itf.alternates.map(a => ({ n: a.alternateSetting, name: a.interfaceName || "" }));
    await dev.claimInterface(this.iface);
    // The names of the targets and the block size are in the configuration descriptor: the
    // name as a string index per alternate setting (the browser does not always fill in
    // interfaceName; seen with Chrome on the device: all names empty), wTransferSize in the
    // DFU functional descriptor (type 0x21).
    try {
      const r = await dev.controlTransferIn({ requestType: "standard", recipient: "device", request: 6, value: 0x0200, index: 0 }, 1024);
      const b = new Uint8Array(r.data.buffer, r.data.byteOffset, r.data.byteLength);
      for (let i = 0; i + 1 < b.length && b[i] > 0; i += b[i]) {
        if (b[i + 1] === 0x21 && b[i] >= 7) this.transfer = b[i + 5] | (b[i + 6] << 8);
        if (b[i + 1] === 4 && b[i] >= 9 && b[i + 2] === this.iface) {
          const alt = this.alts.find(a => a.n === b[i + 3]);
          if (alt && !alt.name && b[i + 8]) alt.name = await this.string(b[i + 8]);
        }
      }
    } catch (e) { /* keep what we have */ }
    // U-Boot's own order (dfu_alt_info in tools/dfu-install/uboot/files/nest_j49.env), for a
    // device that gives no names at all
    const known = ["installer", "rootfs", "boot0", "ring"];
    if (this.alts.length === known.length && this.alts.every(a => !a.name)) this.alts.forEach((a, i) => a.name = known[i]);
    if (this.transfer < 64 || this.transfer > 65535) this.transfer = 4096;
    return this.alts.map(a => a.name);
  }
  /** A string descriptor (US English), as text. */
  async string(index) {
    const r = await this.dev.controlTransferIn({ requestType: "standard", recipient: "device", request: 6, value: 0x0300 | index, index: 0x0409 }, 255);
    const b = new Uint8Array(r.data.buffer, r.data.byteOffset, r.data.byteLength);
    let s = "";
    for (let i = 2; i + 1 < Math.min(b.length, b[0]); i += 2) s += String.fromCharCode(b[i] | (b[i + 1] << 8));
    return s;
  }
  async out(request, value, data) {
    const r = await this.dev.controlTransferOut({ requestType: "class", recipient: "interface", request, value, index: this.iface }, data);
    if (r.status !== "ok") throw new Error("DFU request " + request + ": " + r.status);
  }
  async status() {
    const r = await this.dev.controlTransferIn({ requestType: "class", recipient: "interface", request: REQ.GETSTATUS, value: 0, index: this.iface }, 6);
    if (r.status !== "ok" || r.data.byteLength < 6) throw new Error("DFU status: " + r.status);
    const d = r.data;
    return { status: d.getUint8(0), poll: d.getUint8(1) | (d.getUint8(2) << 8) | (d.getUint8(3) << 16), state: d.getUint8(4) };
  }
  /** Wait until the device leaves the busy states; returns the state it is in. */
  async settle(busy) {
    for (;;) {
      const s = await this.status();
      if (s.state === STATE.ERROR) {
        await this.out(REQ.CLRSTATUS, 0);
        throw new Error("the device reports DFU error " + s.status);
      }
      if (!busy.includes(s.state)) return s.state;
      await sleep(Math.min(Math.max(s.poll, 1), 5000));
    }
  }
  /** Send `data` to the target with this name. progress(done, total). */
  async download(name, data, progress = () => {}) {
    const alt = this.alts.find(a => a.name === name);
    if (!alt) throw new Error('the device has no target "' + name + '" (it has: ' + this.alts.map(a => a.name).join(", ") + ")");
    await this.dev.selectAlternateInterface(this.iface, alt.n);
    let s = await this.status();
    if (s.state === STATE.ERROR) { await this.out(REQ.CLRSTATUS, 0); s = await this.status(); }
    if (s.state !== STATE.IDLE) { await this.out(REQ.ABORT, 0); s = await this.status(); }
    if (s.state !== STATE.IDLE) throw new Error("the device is not ready for a download (state " + s.state + ")");
    let block = 0;
    for (let i = 0; i < data.length; i += this.transfer) {
      await this.out(REQ.DNLOAD, block++ & 0xffff, data.subarray(i, Math.min(i + this.transfer, data.length)));
      await this.settle([STATE.DNLOAD_SYNC, STATE.DNBUSY]);
      progress(Math.min(i + this.transfer, data.length), data.length);
    }
    // an empty block ends the download; the device then writes what it still holds (manifest)
    await this.out(REQ.DNLOAD, block & 0xffff, new Uint8Array(0));
    const end = await this.settle([STATE.DNLOAD_SYNC, STATE.DNBUSY, STATE.MANIFEST_SYNC, STATE.MANIFEST]);
    if (end !== STATE.IDLE && end !== STATE.MANIFEST_WAIT && end !== STATE.DNLOAD_IDLE) throw new Error("download of " + name + " ended in state " + end);
    this.log(name + ": " + data.length + " bytes");
  }
  /** Leave DFU: U-Boot goes on with its boot command. The device drops off the bus. */
  async detach() {
    try { await this.out(REQ.DETACH, 1000); } catch (e) { /* it may be gone before it answers */ }
    // The device is not closed here: it restarts at this moment, and the browser lets go of
    // it by itself when it leaves the bus. (Chrome crashed two seconds after a detach followed
    // by close() on 27 November 2026; whether that was the cause is not known.)
  }
}
