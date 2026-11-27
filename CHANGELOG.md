# Changelog

Newest first. Hardware status of the DFU installer lives in
`tools/dfu-install/README.md` (section "Status") -- start there to continue.

## 2026-11-23 (later) -- USB console from Linux after a cold boot

- Root cause of every "dead after rescue flash / after reset" start: stale DRAM in the DEBUG_LL ring header
  (stock rescue kernel framebuffer) -> decompressor hang. `nandboot` now clears it.
- `nestusb` (new U-Boot command) primes the USB gadget/PHY before `bootm`; Linux then enumerates `g_serial`
  (`/dev/ttyACM0`, root shell) without a prior U-Boot console session.
- `wdtarm` enables the WDT2 clocks first; progress markers at 0x83ef0f00.
- Verified: rescue flash -> Linux + USB console; `reboot` -> Linux + USB console again.

## 2026-11-23 -- DFU installer: stages 1-3 done, Linux boots from NAND by itself

- U-Boot: hush parser was off (`bootz addr || reset` got `||` as FDT address), L2 cache off,
  USB-ACM debug console + one-shot RAM flags, `jumpz` debug command, boot policy `nandboot || usbinstall`
  with WDT2 safety net. Probes 10-26 and their patches are kept in `uboot/probes/`.
- installer: DTB variant (writable root0/boot0 + staging reservation), `mtdflash.rs` fixes
  (u32 ioctl, LE jffs2 magic, page-padded last write, `--verify-mem`), watchdog reset instead of `reboot -f`.
- A-slot production kernel: built-in cmdline `root=/dev/mtdblock6`, DTB with writable root0.
- Result: root0 verified byte for byte, boot0 CRC verified, cold boot and `reboot` come back into Linux
  (console + login on the panel). Open: USB gadget after a plain cold boot, rootfs firmware (wlcore).

## 2026-11-21 -- DFU installer: session 1 transport works on hardware

**Done**
- Session 1 via the stock 2.6.37 rescue kernel (ROM push, `session1-stock.sh`)
  writes spl0+spl1 through the stock NAND driver; read back byte-identical.
- Restore to stock proven through the same route (`nest-restore-uImage`, spl
  images truncated to 245,176 B). Device is back on stock (`2464:0002`).
- `installer/build-stock-kernel.sh` fixed (see below) and made reusable:
  `UBIMG`, `UBIMG1`, `OUT` override the images and the output.
- Unmodified `rescueUImage` proved the ROM/x-load/stock-u-boot chain.

**Fixed (root causes of the failed first attempts)**
- initramfs extracted as a normal user dropped all 97 device nodes
  (no `/dev/console`, no ttyGS0 shell): now repacked as root, node count checked.
- `libgcc_s.so.1` missing in the stock initramfs (`flash_eraseall`/`nandwrite`
  would not start): added from the 2008q3 toolchain, all NEEDED libs checked.
- `nestinstall` ran after `${INITDIR}/start`, which blocks `rcS`: now before it.
- `rm linux/usr/initramfs_data.*` removed the tracked `initramfs_data.S`.
- A restore payload with 2 x 1.5 MB padded images never reached userspace on
  64 MB RAM: keep the payload near 7.2 MB (truncate the images).

**Open**
- **Our U-Boot 2024.07 `nest-j49` crashes/resets early** (ROM `d00e` back every
  ~4 s, black panel), also when pushed to RAM at 0x80100000 with the stock
  x-load. The NAND chain, ECC and uImage header are therefore ruled out. Next:
  probe builds visible over USB (UART is not reachable); hypotheses in the README.
- Stages 2/3 (installer kernel, boot0 over DFU) not exercised: they need
  `0451:df10`.
- `restore-stock.sh` and `flashstub/` are flashstub based and unproven
  (the flashstub hangs before its own error paths). Use the rescue route.

**Operational lessons**
- Start the loader before the DFU gesture; the ROM window is short.
- A device looping into `d00e` can be pushed to without a new gesture.
- Check hub/cable first when no USB device shows (see project memory).

## Earlier (2026-10 / 2026-11)

See `git log`: Yocto `nest-j49` BSP, kernel first light, u-boot env writer,
jffs2 root image, nest-screen UI, DFU installer host/build side
(U-Boot image byte-faithful to the stock spl0 header, 64 MB-safe rootfs
transport, stage scripts).
