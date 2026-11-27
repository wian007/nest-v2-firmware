# OMAP3 USB peripheral-boot helper (Nest J49 "diamond")

The J49 OMAP3 is a **GP device**, so its mask-ROM accepts **unsigned** images
over USB peripheral boot. This is the "DFU"/recovery path: load a first stage
(x-loader/MLO or u-boot SPL) over USB with no UART and no NAND writes, which
makes the board effectively unbrickable.

## Setup (NixOS)
Everything is in the flake dev shell:
```
nix develop
```
For non-root USB access (so the script runs WITHOUT sudo), install a udev rule
granting the OMAP3 mask-ROM device to your user — one-time:

- **NixOS:** merge `tools/omap-usbboot/nixos-udev.nix` into
  `/etc/nixos/configuration.nix`, then `sudo nixos-rebuild switch`.
- **Other distros:** `sudo cp tools/omap-usbboot/99-omap-usbboot.rules
  /etc/udev/rules.d/ && sudo udevadm control --reload`.

Without the rule, run the script with `sudo -E` instead.

## Use
```
# 1) prove the handshake + identify the SoC (no payload, safe):
python3 tools/omap-usbboot/omap-usbboot.py id

# 2) later, push a first stage over USB:
python3 tools/omap-usbboot/omap-usbboot.py boot path/to/MLO

# just watch for the ROM device:
python3 tools/omap-usbboot/omap-usbboot.py watch
```
Start the command, then make the OMAP3 ROM enter peripheral boot (power-cycle;
the ROM opens a short USB window). The tool polls for VID `0451` PID `d00e`.

## Entering peripheral boot
The ROM boots NAND first when it is valid, so the USB window may be skipped.
Ways to force it (to be determined for the J49):
- power-cycle while the tool waits and hope the ROM polls USB in its boot list;
- a button/test-pad that pulls a sys_boot pin at reset;
- as a last resort, temporarily invalidating NAND boot (risky — not needed since
  we have a full backup, but avoid until the USB path is proven).

Protocol: GET_ID = 0xF0030003 (returns ASIC ID); BOOT = 0xF0030002 then 4-byte
LE size then image. See omap-usbboot.py.
