# The image files of the J49 installer: who made them, licences, sources

These files are what the installer page loads onto a Nest Learning Thermostat of the 2nd
generation. They are free software. Each is listed with its makers, its licence and the place
where its source code is; the checksums are at the end. Nothing here was made by or with the
maker of the thermostat, and none of the people named is responsible for this installer.

## The loaders of the USB boot chain

| File | What | Made by | Licence | Source |
|---|---|---|---|---|
| `x-load.bin` | First loader: TI x-loader with a USB download handler. The boot ROM runs it from RAM. | Texas Instruments; the USB handler by GTVHacker (NestDFUAttack), in the variant of ajb142 | GPL-2.0 | <https://github.com/cuckoo-nest/x-loader>, <https://github.com/gtvhacker/NestDFUAttack> |
| `u-boot.bin` | U-Boot 2012.10 for this board, run from RAM only to start the service kernel. | The U-Boot project; board support from the open-source release for the thermostat; build by GTVHacker / cuckoo-nest | GPL-2.0-or-later | <https://github.com/cuckoo-nest/u-boot> |
| `nest-service-uImage` | The service kernel: Linux 2.6.37 for this board, with an initramfs of this project: `nandio` (backup, boot loader, restore), BusyBox and three libraries of the GNU C Library. | The Linux kernel developers; board support from the open-source release for the thermostat; BusyBox; GNU C Library; `nandio` by this project | Kernel GPL-2.0; BusyBox GPL-2.0; glibc LGPL-2.1-or-later; `nandio` MIT | <https://github.com/cuckoo-nest/linux>, <https://busybox.net>, <https://sourceware.org/glibc>, `tools/dfu-install/nandio` and `installer/build-service-kernel.sh` in this project |

The three are built with <https://github.com/cuckoo-nest/cuckoo_loader>, after GTVHacker's
NestDFUAttack (2014), which found that the boot ROM of the thermostat accepts code over USB.

## The system that is installed

| File | What | Licence | Source |
|---|---|---|---|
| `u-boot-nest-j49.uimg` | U-Boot 2024.07 with our board support | GPL-2.0-or-later | <https://source.denx.de/u-boot/u-boot> plus `tools/dfu-install/uboot` in this project |
| `uImage-j49-a` | Linux 6.6 with our device tree and patches, and `nandio` as its first program: it starts the system, or waits for the installer when there is none | GPL-2.0; `nandio` MIT (linked with musl, MIT) | <https://kernel.org> plus `meta-j49/recipes-kernel` and `tools/dfu-install` in this project |
| `*.rootfs.jffs2` | The root file system, built with the Yocto Project: many packages, each under its own licence (the build writes the list to `build/tmp/deploy/licenses`) | various free software licences | `meta-j49` and `kas/nest-j49.yml` in this project build it from the sources of each package |

This project: <https://github.com/wian007/nest-v2-firmware> (our own code MIT). To get the
source of exactly these builds, or if a file is listed wrongly, open an issue there.

The licence texts: GPL-2.0 <https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt>,
LGPL-2.1 <https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt>, MIT in `LICENSE` of this
project.

## The files here

| File | Size | sha256 |
|---|---|---|
| `j49-image-nest-j49.rootfs.jffs2` | 21548920 | `c2df34406754392b3ec0c8c572ebd2c1b3fe1e77a859522319f99744ebb19ba0` |
| `nest-service-uImage` | 2831164 | `e2e6e713325b313a8a0343bc3e5d475ee952dc3aa74eabcf4318f8d202715860` |
| `u-boot.bin` | 246572 | `52aa039f5b36d8cd5108e6bbb69f5dbd60a5c3750404c8d3fbc721d53721ccf0` |
| `u-boot-nest-j49.uimg` | 548584 | `ae7a0fd748eb7671626593a56a00396f8494296d0f76a4f2528f29444d221cd2` |
| `uImage-j49-a` | 4943541 | `01dc8afa6576925a4c9230f7aede5db3d16fe82a4f22848a7ca4e197251380a2` |
| `x-load.bin` | 27876 | `aee635f0d09180677fa86fea6797b1ad36f64b112f7eea3a5572ab7f28fd5c21` |
