# Installation guide

How to put the J49 software on a Nest Learning Thermostat of the 2nd generation, and how to go
back. Read the [disclaimer](../DISCLAIMER.md) first: this replaces the firmware and the device
switches real heating.

**Status of this guide (27 November 2026).** The steps of the install are the ones we did by
hand with command-line tools, several times, on one thermostat. The installer page was tried on
that thermostat up to and including a full backup: the loaders, the service kernel with our own
screen, the check that it is a 2nd generation thermostat, and a backup of the whole flash
memory that checks out (256 MB in about 9 minutes). **Not yet done from the page on a device:**
writing the boot loader, installing the system, and putting a backup back. Until this paragraph
says otherwise, expect to need the command-line route in
[tools/dfu-install/README.md](../tools/dfu-install/README.md) when something does not work.

## What you need

- A Nest Learning Thermostat, **2nd generation** (board "J49", shown as Display-2.x under
  Settings > Technical info). Other models have other hardware; do not try this on them.
- A USB cable that carries data (micro-USB; many cables only charge) and a computer with
  **Chrome or Edge** (Windows, Linux, macOS, ChromeOS). Firefox and Safari have no WebUSB.
- The image files (see "The files" below).
- About half an hour, and 200 MB of free disk space for the backup.

Operating system notes:

- **Windows:** the boot ROM of the thermostat and our U-Boot have no driver. Install WinUSB
  for each with [Zadig](https://zadig.akeo.ie/), once: with the device in that mode on USB,
  "Options > List All Devices", select "OMAP3630" (USB ID `0451 D00E`) or "USB download gadget"
  (`0451 DF10`), choose WinUSB, "Install Driver". The ROM mode lasts seconds: start Zadig first
  and be quick, or repeat the ring gesture.
- **Linux:** give your user access to the two devices, for example with
  `tools/dfu-install/99-nest-dfu.rules` in `/etc/udev/rules.d/`, and be in the group that owns
  serial ports (`dialout` or `uucp`).
- **macOS, ChromeOS:** nothing to install.

## The installer page

The page is in `tools/web-installer`. It needs https or localhost (a browser allows USB only
there). To run it yourself:

    tools/web-installer/build-site.sh /tmp/j49-site            # add --images when you built them
    python3 -m http.server 8049 -d /tmp/j49-site
    # open http://localhost:8049/

It has three jobs: **Install** (backup, then install), **Backup only** (changes nothing) and
**Restore** (put a backup back). The page is in English and Dutch.

## The steps of an install

1. **Files.** The page loads the images that were published with it, or asks for each file.
   It checks the sha256 of each file when it knows one.
2. **Boot mode.** Take the thermostat off the wall and connect it to the computer. Click
   "Connect": the browser opens a list. Now hold the ring pressed for 10 to 15 seconds, until
   the thermostat restarts. For a few seconds it is then a USB device "OMAP3630"; select it in
   the list. The page sends a loader, a small U-Boot and a rescue kernel into RAM. Nothing is
   written to the thermostat by this. The screen shows the Nest logo and then text.
3. **Service.** The rescue kernel shows up as a USB serial port ("Gadget Serial"). Click the
   button and select it. The page now talks to a small program in that kernel (`nandio`).
4. **Backup.** The page reads the whole flash memory (256 MB, less in the file because empty
   blocks are not stored) into one `.j49backup` file and checks it. **Keep this file.** It
   holds the firmware of *your* thermostat, with its keys and its WiFi data: do not publish it.
5. **Boot loader.** Our U-Boot is written into both boot loader partitions and read back. From
   here on the thermostat does not start the stock firmware any more. It restarts; the screen
   stays dark.
6. **System.** The thermostat is now a USB device "USB download gadget". Select it. The page
   sends the installer and the root file system (about 27 MB) into RAM; the thermostat then
   writes it to its flash memory and shows the progress on its own screen (a few minutes).
   When it is done it comes back on USB by itself and the page writes the kernel. The
   thermostat then starts the J49 software.

If a step fails, the page says so and the step can be done again. The thermostat cannot be
bricked by a failed step as long as the first loader partition (`ipl`) is intact, and nothing
in this install writes it: the boot mode of step 2 is in the ROM of the processor and always
works. That was tried several times, also with a boot loader that did not start.

## After the install

- Heating is **off** until you switch it on, and sleeping is off until you enable it.
- **WiFi.** The first network is set over USB. Connect the thermostat to the computer; it is a
  serial console (115200 baud; `screen /dev/ttyACM0 115200` on Linux, PuTTY on Windows). Log in
  as `root` and write the network into `/etc/wpa_supplicant/wpa_supplicant-wlan0.conf`:

      network={
          ssid="the name"
          psk="the password"
          key_mgmt=WPA-PSK
      }

  then `systemctl restart wpa_supplicant@wlan0`. After that the web page of the thermostat
  (`http://<its address>/`) adds and removes networks.
- Set a root password or an SSH key if your network needs it: the image comes with an open
  root account, meant for a home network.
- Everything else: [thermostat.md](thermostat.md), [home-assistant.md](home-assistant.md).

## Back to stock

Choose **Restore** on the page and give it the backup file of this thermostat. Steps 1 to 3
are the same; the page then writes every partition of the backup back, except `ipl`, and
restarts the thermostat. A backup is written back block for block, so use it only on the
thermostat it was made on.

## The files

The installer page on <https://wian007.github.io/nest-v2-firmware/> loads the image files by
itself. The same files, with checksums and a notice of makers, licences and sources, are a
[release](https://github.com/wian007/nest-v2-firmware/releases) of the project. To build them
yourself:

| File | What | From |
|---|---|---|
| `x-load.bin`, `u-boot.bin` | The loaders of the USB boot chain | [tools/dfu-install/nest-dfu](../tools/dfu-install/nest-dfu/README.md) |
| `nest-service-uImage` | Rescue kernel with the service program `nandio` | `tools/dfu-install/installer/build-service-kernel.sh` |
| `u-boot-nest-j49.uimg` | Our U-Boot 2024.07 | `tools/dfu-install/uboot/build.sh` |
| `zImage-installer`, `uImage-j49-a` | Installer kernel and the kernel | `tools/dfu-install/installer/build.sh` |
| `j49-image-nest-j49.rootfs.jffs2` | The root file system | the Yocto build (README, "Build") |

## How it works, for who wants to check it

- `tools/web-installer/js/omap.js`: the protocol of the OMAP3 boot ROM and the x-loader, after
  `tools/omap-loader`.
- `tools/dfu-install/nandio/nandio.c`: the service program, with the protocol at the top.
  It reads and writes through the MTD driver of the stock kernel, so what it writes has the
  ECC format the stock boot chain reads. `nandio/test.py` tests it on a PC.
- `tools/web-installer/js/service.js`: the browser side, and the format of the backup file.
  `tools/dfu-install/nandio/j49backup.py` checks a backup file on a PC and converts it to and
  from plain images; `probe.py` talks to the service without the browser.
- `tools/web-installer/js/dfu.js`: USB DFU 1.1 download to our U-Boot.
- `node tools/web-installer/test/run.mjs tools/dfu-install/nandio/nandio-host` runs the
  browser code against a simulated ROM, a simulated DFU device and the real `nandio`.
