# omap-loader -- OMAP3 ROM USB peripheral-boot pusher

Host-side tool to push a first-stage image to the Nest J49's OMAP3 mask-ROM
over USB and have it jump to it. The J49 is a **GP (General Purpose)** part,
so the ROM accepts **unsigned** images here -- this is the same unbrickable
USB-DFU path `tools/omap-usbboot` and `tools/uboot-recovery` use, but this
tool can push *multiple* files to explicit addresses and jump, which is what
a full x-load -> u-boot -> kernel chain needs (`tools/omap-usbboot.py` only
pushes one blob).

`omap_loader.c` is vendored, unmodified, from
<https://github.com/ajb142/omap_loader> (`send_correct_jump_for_nest` branch,
commit `9cbf5f4`), GPLv2 (see `LICENSE.md`). That branch's one-line patch
("jump command") is specifically required by the Nest J49's x-loader -- the
upstream `master` branch sends a jump command that x-load doesn't recognize.

## Build

```
tools/omap-loader/build.sh
# -> tools/omap-loader/out/omap_loader
```
Builds with a Nix toolchain (gcc + libusb1 via pkg-config) -- no manual
patchelf/interpreter hacks, no stray glibc store paths.

## Use

This is pure RAM boot via the mask-ROM: it does not itself write anything to
NAND. What runs once it jumps can still write to flash if *it* chooses to
(e.g. mounting a root partition and editing a file) -- that part is on you.

```
tools/omap-loader/push.sh x-load.bin u-boot.bin uImage
```
Then:
1. Plug the Nest into USB.
2. Hold the display for 10-15s until it reboots -- this opens the ROM's short
   USB peripheral-boot window (shows up as `0451:d00e` in `lsusb`).
3. `push.sh` uploads x-load to the ROM's fixed load address, u-boot.bin to
   `0x80100000`, the kernel to `0x80A00000`, then jumps to `0x80100000`.

No root/sudo needed if the `tools/omap-usbboot` udev rule
(`99-omap-usbboot.rules` / `nixos-udev.nix`, same `0451:d00e` VID:PID) is
installed -- it already covers this tool too.

## Getting x-load.bin / u-boot.bin / a kernel

These are not built here (they're not generic -- x-load needs a
J49-specific config, and the kernel is whatever you want to boot). `x-load.bin`
and `u-boot.bin` as the installer uses them are in `tools/dfu-install/nest-dfu/`
(see its README for who made them). The installer page (`docs/install.md`) does
the same push from the browser. Other sources:

- GTVHacker's NestDFUAttack (<https://github.com/gtvhacker/NestDFUAttack>):
  `Release/Nest/{x-load.bin,u-boot.bin,uImage}` are prebuilt
  for the J49 (`j49-usb-loader_config`). `Release/Nest/rescueUImage` is a
  second kernel target from the same build. The initramfs these kernels boot
  (`Dev/root/etc/init.d/rootme`) has been pared down to just start a root
  shell on the USB gadget console (`ttyGS0`/`ttyGS0` -> `/dev/ttyACM*` on the
  host) -- no dropbear, no network, no cloud/config edits (this was done in a
  copy of that tree that is not published). That's what makes
  it safe to use for a quick recovery shell: it's RAM-only until *you*
  explicitly mount and write something.
- `tools/kernel-firstlight/build.sh` builds our own mainline 6.6 kernel +
  DTB + busybox initramfs (`zImage-dtb-kx` is the combined blob with appended
  DTB, header length patched for this exact use). Point `push.sh`'s third
  argument at that instead of `uImage` to RAM-boot mainline over DFU, with no
  stock involvement at all.

## Recovering SSH access on stock (what this was built for, 2026-11-21)

Stock's own `/etc/init.d/sshd` never actually starts anything: it execs
`${USRSBINDIR}/sshd` (`/usr/sbin/sshd`), which doesn't exist on this device
-- `dropbear`/`dropbearmulti` are present under `/bin/` with host keys
already generated under `/etc/dropbear/`, just never wired up to the init
script. From a `rootme`-style recovery shell:

```
mkdir -p /mnt/root0
mount -t jffs2 /dev/mtdblock7 /mnt/root0   # mtd7 = root0, see /proc/mtd
# add your pubkey (append, don't clobber an existing one):
cat >> /mnt/root0/root/.ssh/authorized_keys <<'EOF'
ssh-ed25519 AAAA... you@host
EOF
chmod 600 /mnt/root0/root/.ssh/authorized_keys
# wire sshd up to dropbear:
cat > /mnt/root0/usr/sbin/sshd <<'EOF'
#!/bin/sh
exec /bin/dropbear -F "$@"
EOF
chmod 755 /mnt/root0/usr/sbin/sshd
sync; umount /mnt/root0
```
Then power-cycle back into stock. `mtd7`/`root0` is the **A-slot, the
fallback rootfs** -- only ever mount it read-write for a deliberate,
understood change like this, never experimentally.
