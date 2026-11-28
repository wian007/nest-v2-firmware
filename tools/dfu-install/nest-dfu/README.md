# nest-dfu: the loader images of the USB boot chain

The install starts the thermostat over USB with two loaders, a TI x-loader with a USB download
handler and a U-Boot 2012.10 for the board that runs from RAM, and then the service kernel
(`../payload/nest-service-uImage`). The two loaders are **in this directory**, as they were in
the last release. They are GPL-2.0 software from the NestDFUAttack work of GTVHacker; the
cuckoo-nest project keeps the sources and a build script:

- https://github.com/gtvhacker/NestDFUAttack
- https://github.com/cuckoo-nest/cuckoo_loader (`build.sh` builds x-loader, U-Boot and the kernel
  from https://github.com/cuckoo-nest/x-loader, `u-boot` and `linux`)

Who made each image, its licence and where its source is: `../../web-installer/images-notice.md`.
`./check.sh` compares the files with the checksums the install was tested with. An image you
built yourself will have another checksum; whether such an image works here was not tested.

The two kernels in the table (`rescueUImage`, `uImage`) are those of NestDFUAttack. The install
does not use them any more and they are not in this repository; they are named for the notes
below, which are about the first, command-line route.

| file | size | sha256 | role |
|---|---|---|---|
| `x-load.bin` | 27,876 | `aee635f0d09180677fa86fea6797b1ad36f64b112f7eea3a5572ab7f28fd5c21` | TI x-loader fork (ajb142/omap_loader `send_correct_jump_for_nest` branch) with a USB download handler; the ROM pushes it to SRAM and it receives the next files at explicit addresses and jumps. |
| `u-boot.bin` | 246,572 | `52aa039f5b36d8cd5108e6bbb69f5dbd60a5c3750404c8d3fbc721d53721ccf0` | Stock U-Boot 2012.10 "diamond/hass" build used only as the RAM bootloader of the chain. Its default env has `bootcmd=run nfsboot || reset`, `nfsboot=run addtty addmodel && bootm 0x80a00000`, bootargs `console=ttyO0 console=ttyGS0 console=tty1`. Raw binary (headerless) -- x-load jumps to it directly. |
| `rescueUImage` | 6,966,340 | `add18697d358c8d15f6b8ad0dcd6a6cb926ecb6e6a9ce490573b91c0e148355f` | The PROVEN stock 2.6.37 rescue kernel: vendor display (logo), fbcon on the panel, g_serial console on ttyGS0, and an initramfs with a root shell + `usr/sbin/flash_eraseall`/`nandwrite`. This is the control image for session-1 diagnostics. |
| `uImage` | 6,964,684 | `58f4c20189cb801773a11e7cd81f5db67c54ac36ee180cc413972950164731a0` | The same stock kernel image as shipped (`uImage` vs `rescueUImage` differ by packaging/metadata). Keep for reference. |

Usage in this repo. This is the first, command-line route and it is history: the installer
page (`docs/install.md`) sends the two loaders and `../payload/nest-service-uImage` itself. The
first command needs `rescueUImage`, which is not in this repository.

```sh
# proven chain, control test (logo + ttyGS0 shell):
tools/omap-loader/out/omap_loader \
  -f tools/dfu-install/nest-dfu/x-load.bin \
  -f tools/dfu-install/nest-dfu/u-boot.bin -a 0x80100000 \
  -f tools/dfu-install/nest-dfu/rescueUImage  -a 0x80A00000 \
  -v -j 0x80100000

# session 1 with OUR auto-flash initramfs (rebuilt kernel):
tools/dfu-install/session1-stock.sh
#   -> uses payload/nest-install-uImage; override:
#   KERNEL=tools/dfu-install/nest-dfu/rescueUImage session1-stock.sh   (control)
```

The `Dev/` tree (stock kernel source, initramfs, 2008q3 toolchain) is NOT
vendored: `installer/build-stock-kernel.sh` reads it from the external repo
(`DFU_ATTACK=...`) and builds inside that repo's Docker environment.

## Notes (2026-11-21)

(History of the command-line route. The remark about the short ROM window is still true.)

The chain was re-proven on hardware: pushing `x-load.bin` + `u-boot.bin` +
`rescueUImage` gives the logo and a root shell on `/dev/ttyACM0` (`0525:a4a7`).
The unmodified rescue kernel lacks `libgcc_s.so.1`, so `flash_eraseall`,
`nandwrite`, `nanddump` and `ps` do not run in it; our rebuilt payloads add it
(see `../installer/build-stock-kernel.sh`). The ROM window after power-up is
short: start the loader first, then do the DFU gesture.
