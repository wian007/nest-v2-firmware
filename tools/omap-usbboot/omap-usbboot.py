#!/usr/bin/env python3
"""
OMAP3 ROM USB peripheral-boot helper for the Nest J49 ("diamond").

The OMAP3 mask-ROM, when it enters USB peripheral boot, enumerates as a TI
device (VID 0x0451, PID 0xd00e for OMAP3). It speaks a tiny protocol:

  - GET_ID  (0xF0030003): host writes the 4-byte opcode, ROM returns its
    ASIC ID block (device type GP/HS/EMU, chip id, ROM version). No payload
    needed -> safe way to prove peripheral boot works and identify the SoC.
  - BOOT    (0xF0030002): host writes opcode, then 4-byte image size (LE),
    then the image. ROM copies it to public SRAM and jumps to it. Use this to
    push a first-stage (x-loader/MLO or u-boot SPL) with no UART/NAND.

Because the J49 OMAP3 is a GP (General Purpose) device, the ROM accepts
UNSIGNED images here -> this is the unbrickable recovery/load path.

Usage:
  sudo -E python3 omap-usbboot.py id            # wait for ROM, read ASIC ID
  sudo -E python3 omap-usbboot.py boot MLO      # wait for ROM, push 1st stage
  python3 omap-usbboot.py watch                 # just watch for the ROM device

Run `watch`/`id` in a terminal, then power-cycle the Nest so the ROM's
peripheral-boot window opens.
"""
import sys, time, struct

# TI OMAP ROM peripheral-boot USB IDs (OMAP3 = 0xd00e; others kept for safety)
TI_VID = 0x0451
ROM_PIDS = {0xd00e, 0xd00f, 0xd010, 0xd011, 0xd012, 0x6141}

MSG_GET_ID = 0xF0030003
MSG_BOOT   = 0xF0030002


def find_rom(usb):
    for pid in ROM_PIDS:
        dev = usb.core.find(idVendor=TI_VID, idProduct=pid)
        if dev is not None:
            return dev, pid
    return None, None


def wait_for_rom(usb, timeout=None):
    print("waiting for OMAP ROM peripheral-boot device "
          "(VID 0451, PID d00e...)  -- power-cycle the Nest now", flush=True)
    start = time.time()
    while True:
        dev, pid = find_rom(usb)
        if dev is not None:
            print(f"found OMAP ROM: {TI_VID:04x}:{pid:04x}", flush=True)
            return dev, pid
        if timeout and (time.time() - start) > timeout:
            return None, None
        time.sleep(0.05)


def endpoints(dev):
    import usb.util
    # The ROM may already be configured, or the kernel usbfs may hold it; be
    # tolerant so a second attempt on the same enumeration still works.
    try:
        if dev.is_kernel_driver_active(0):
            dev.detach_kernel_driver(0)
    except Exception:
        pass
    try:
        dev.set_configuration()
    except Exception:
        pass
    cfg = dev.get_active_configuration()
    intf = cfg[(0, 0)]
    ep_out = usb.util.find_descriptor(
        intf, custom_match=lambda e: usb.util.endpoint_direction(
            e.bEndpointAddress) == usb.util.ENDPOINT_OUT)
    ep_in = usb.util.find_descriptor(
        intf, custom_match=lambda e: usb.util.endpoint_direction(
            e.bEndpointAddress) == usb.util.ENDPOINT_IN)
    return ep_out, ep_in


def parse_asic_id(data):
    """Best-effort parse of the OMAP ASIC ID block."""
    print(f"ASIC ID ({len(data)} bytes):")
    print("  " + " ".join(f"{b:02x}" for b in data))
    if not data:
        return
    n = data[0]
    print(f"  subblocks: {n}")
    # The ID subblock encodes secure/device type. GP is what we expect.
    # Scan for the device-type byte (TST=0x0f? EMU/HS/GP vary by ROM); we mainly
    # care that we got a coherent, non-empty response == handshake works.
    types = {0x01: "TST", 0x02: "EMU", 0x03: "HS", 0x04: "GP"}
    for off in range(1, min(len(data), 16)):
        if data[off] in types:
            print(f"  possible device-type byte @%d: 0x%02x (%s)"
                  % (off, data[off], types[data[off]]))
            break


def cmd_id(usb):
    dev, pid = wait_for_rom(usb)
    ep_out, ep_in = endpoints(dev)
    ep_out.write(struct.pack("<I", MSG_GET_ID))
    time.sleep(0.05)
    data = bytes(ep_in.read(512, timeout=2000))
    parse_asic_id(data)


def cmd_boot(usb, path):
    with open(path, "rb") as f:
        img = f.read()
    print(f"image {path}: {len(img)} bytes")
    dev, pid = wait_for_rom(usb)
    ep_out, ep_in = endpoints(dev)
    # OMAP3 USB peripheral boot: the ROM sends its ASIC ID UNSOLICITED on
    # connect; the host must READ it first, THEN send the boot message, the
    # 4-byte image size and the image. (Writing the opcode first desyncs the
    # ROM and the size write NAKs.)
    try:
        asic = bytes(ep_in.read(512, timeout=3000))
        print(f"ASIC ID ({len(asic)} bytes): "
              + " ".join(f"{b:02x}" for b in asic[:16]) + " ...")
    except Exception as e:
        print(f"(no ASIC ID read: {e}) -- continuing anyway")
    # The OMAP ROM STALLs if USB data arrives too fast: on modern xHCI hosts
    # several packets get packed into one frame and the ROM aborts the endpoint
    # (LIBUSB_ERROR_PIPE). Pace every OUT transfer with a ~1 ms delay and use
    # small 512-byte chunks. (Fix per downtowndougbrown.com's omap_loader writeup.)
    time.sleep(0.001)
    ep_out.write(struct.pack("<I", MSG_BOOT), timeout=3000)
    time.sleep(0.001)
    ep_out.write(struct.pack("<I", len(img)), timeout=3000)
    mv = memoryview(img)
    CH = 512
    sent = 0
    for i in range(0, len(img), CH):
        time.sleep(0.001)
        try:
            n = ep_out.write(mv[i:i + CH], timeout=5000)
            sent += n if n else len(mv[i:i + CH])
        except usb.core.USBError as e:
            print(f"write stopped after {sent}/{len(img)} bytes: {e}", flush=True)
            if sent >= 2048:
                print("  (>=2KB sent: ROM likely consumed the header-specified "
                      "image and JUMPED -- SPL should be running)", flush=True)
            else:
                print("  (<2KB sent: ROM did not accept the image -- real failure)",
                      flush=True)
            return
        if sent % (CH * 32) == 0:
            print(f"  sent {sent}/{len(img)}", flush=True)
    print(f"image sent in full ({sent} bytes); ROM should jump to it now",
          flush=True)


def cmd_watch(usb):
    dev, pid = wait_for_rom(usb)
    print("device present; run `id` or `boot` next.")


def main():
    import usb.core, usb.util  # pyusb
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    cmd = sys.argv[1]
    if cmd == "id":
        cmd_id(usb)
    elif cmd == "boot":
        if len(sys.argv) < 3:
            print("usage: boot <image>")
            sys.exit(1)
        cmd_boot(usb, sys.argv[2])
    elif cmd == "watch":
        cmd_watch(usb)
    else:
        print(__doc__)
        sys.exit(1)


if __name__ == "__main__":
    main()
