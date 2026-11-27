// The serial port of the thermostat (USB CDC ACM) through WebUSB, for browsers without Web
// Serial: Chrome on Android. Only what the installer needs: bytes in, bytes out.
export const ACM_FILTER = { vendorId: 0x0525, productId: 0xa4a7 };

export class UsbSerial {
  constructor(device) { this.dev = device; }
  async open() {
    const d = this.dev;
    await d.open();
    if (!d.configuration) await d.selectConfiguration(1);
    // the data interface (class 0x0a) has the two bulk endpoints
    let found = null, comm = null;
    for (const itf of d.configuration.interfaces) {
      const alt = itf.alternates[0];
      if (alt.interfaceClass === 0x02) comm = itf.interfaceNumber;
      if (alt.interfaceClass !== 0x0a) continue;
      const i = alt.endpoints.find(e => e.type === "bulk" && e.direction === "in");
      const o = alt.endpoints.find(e => e.type === "bulk" && e.direction === "out");
      if (i && o) found = { n: itf.interfaceNumber, i: i.endpointNumber, o: o.endpointNumber, size: i.packetSize };
    }
    if (!found) throw new Error("this USB device has no serial data interface");
this.ep = found;
    // The control interface first, then the data interface: where the system has a serial
    // driver of its own on the device (Android often has), taking the control interface
    // makes that driver let go of both. Try a few times: the driver needs a moment.
    const claim = async (n, what) => {
      let last;
      for (let i = 0; i < 5; i++) {
        try { await d.claimInterface(n); return; } catch (e) { last = e; await new Promise(r => setTimeout(r, 300)); }
      }
      throw new Error("could not take the " + what + " interface (" + n + ") of the serial port: " + last.message);
    };
    if (comm !== null) { try { await claim(comm, "control"); this.comm = comm; } catch (e) { this.note = e.message; } }
    await claim(found.n, "data");
    // 115200 8N1 and "terminal ready", as a serial driver sends when a program opens the port
    if (this.comm !== undefined) {
      try {
        await d.controlTransferOut({ requestType: "class", recipient: "interface", request: 0x20, value: 0, index: comm }, Uint8Array.of(0, 0xc2, 1, 0, 0, 0, 8));
        await d.controlTransferOut({ requestType: "class", recipient: "interface", request: 0x22, value: 3, index: comm });
      } catch (e) { /* the data flows without it */ }
    }
  }
  /** The next bytes, or null when the device is gone. */
  async pull() {
    if (this.closed) return null;
    for (;;) {
      let r;
      try { r = await this.dev.transferIn(this.ep.i, 16384); } catch (e) { return null; }
      if (r.status === "stall") { try { await this.dev.clearHalt("in", this.ep.i); } catch (e) { return null; } continue; }
      if (r.data && r.data.byteLength) return new Uint8Array(r.data.buffer, r.data.byteOffset, r.data.byteLength);
      if (this.closed) return null;
    }
  }
  async write(data) {
    const r = await this.dev.transferOut(this.ep.o, data);
    if (r.status !== "ok") throw new Error("USB write: " + r.status);
  }
  async close() {
    this.closed = true;
    try { await this.dev.close(); } catch (e) { /* the device is gone */ }
  }
}
