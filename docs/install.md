# Installation guide

How to put the J49 software on a Nest Learning Thermostat of the 2nd generation, and how to go
back. Read the [disclaimer](../DISCLAIMER.md) first: this replaces the firmware and the device
switches real heating.

**Status of this guide (27 November 2026).** Done on thermostats so far:

- A full install from the page, from the ring gesture to the running system, with the WiFi
  setup hotspot coming up after it.
- Putting a backup back from the page: a thermostat on which an earlier install had failed was
  brought back to the stock firmware with its backup.
- Steps from Chrome on an Android phone.
- The same on a second thermostat that still had all its stock data. An earlier version of
  the installer failed there: the new system cannot read the partitions the stock firmware
  wrote (another error correction layout). The installer now fetches the WiFi calibration and
  erases the partition `scratch` while the stock kernel still runs; with that it installed.

Make the backup first: it is what brought that thermostat back. The ring gesture always
brings the thermostat back to step 2.

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
5. **Boot loader.** Our U-Boot is written into both boot loader partitions and read back, and
   the partition for the system is erased. From here on the thermostat does not start the
   stock firmware any more. It restarts; the screen stays dark.
6. **Kernel.** The thermostat is now a USB device "USB download gadget". Select it. The page
   sends the kernel, which U-Boot writes to the flash memory. The thermostat restarts and
   starts that kernel. The kernel finds no system and waits for the installer: the screen shows
   "J49 INSTALLER" again.
7. **System.** The thermostat is a USB serial port again ("Gadget Serial"). Select it. The page
   sends the root file system (about 21 MB); the thermostat writes it block by block and reads
   every block back, with the progress on the page and on its own screen. That takes under a
   minute. The thermostat then restarts and starts the J49 software.

The kernel that is installed starts a small program of ours first, at every start. When the
partition for the system holds a system, it starts that. When it is empty or damaged, the
thermostat waits in installer mode, and step 7 can be done again without the ring gesture.

If a step fails, the page says so and the step can be done again. The thermostat cannot be
bricked by a failed step as long as the first loader partition (`ipl`) is intact, and nothing
in this install writes it: the boot mode of step 2 is in the ROM of the processor and always
works. That was tried several times, also with a boot loader that did not start.

## After the install

- Heating is **off** until you switch it on. Sleeping is on; switch it off on the web page when you do not want it.
- **WiFi.** A thermostat without a network opens a WiFi hotspot of its own, about half a
  minute after it started. Its screen shows the name (`J49-setup-` and four characters) and
  the address. Connect a phone or laptop to that network (it has no password), open
  `http://192.168.4.1/`, choose "Log in" and confirm the code on the thermostat with the ring.
  Add your network in the WiFi block of the page. The hotspot then closes and the thermostat
  joins your network. The hotspot also opens when the thermostat could not join any of its
  networks for five minutes.

  The other way is the USB serial console (115200 baud; `screen /dev/ttyACM0 115200` on Linux,
  PuTTY on Windows). Log in as `root` and write the network into
  `/etc/wpa_supplicant/wpa_supplicant-wlan0.conf`:

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
| `uImage-j49-a` | The kernel, with `nandio` as its first program | `tools/dfu-install/installer/build.sh` |
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
- `tools/web-installer/js/dfu.js`: USB DFU 1.1 download to our U-Boot (the kernel only).
- Why the root file system does not go through U-Boot, and the other things the first installs
  taught, are in the commit messages of 27 November 2026 and at the top of `nandio.c`.
- `node tools/web-installer/test/run.mjs tools/dfu-install/nandio/nandio-host` runs the
  browser code against a simulated ROM, a simulated DFU device and the real `nandio`.
