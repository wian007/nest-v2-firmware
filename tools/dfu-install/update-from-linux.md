# Updating a running J49 without the ring gesture

Works from the running system since `j49-dfu` opens `/dev/mem` with `O_SYNC` (a66b0059).
Done on 2026-11-25/26 for the kernel and for a complete root filesystem.

## Open the U-Boot DFU window from Linux

On the device (stop feeding the hardware watchdog; it cannot be stopped, so it resets the board
within 60 s, a warm reset that keeps DRAM and with it the flag):

    j49-dfu dfu --no-reboot
    busctl set-property org.freedesktop.systemd1 /org/freedesktop/systemd1 \
        org.freedesktop.systemd1.Manager RuntimeWatchdogUSec t 0

`j49-dfu` from an image built before 2026-11-26 21:40 (commits up to ed2df160) does not work:
its `mmap` took a 32-bit offset and the flag went to the wrong page, so the board reset at once
and booted normally (the 7 s gadget, no window). Use the static musl build
(`tools/j49-dfu/target/armv7-unknown-linux-musleabihf/release/j49-dfu`, copied to `/run`) or a
build with the `mmap64` fix.

The real window is the `0451:df10` gadget for which `dfu-util -l` lists the alt `boot0`. The
`0451:df10` that shows for ~7 s on every boot is only U-Boot's USB priming gadget.

## Kernel (boot0)

    nix shell nixpkgs#dfu-util -c bash tools/dfu-install/update-boot0.sh <uImage>

The script waits for the window, writes `boot0`, writes four zero bytes to the `installer` RAM alt
(so a stale zImage there cannot boot) and detaches with `-a installer -e`. dfu-util prints
"can't detach", the board resets anyway and boots boot0. Without `-a` the detach does nothing and
the board stays in DFU.

Build the uImage with `installer/build.sh` after deleting `work-shared/nest-j49/dfu-build/.config`
(it is only copied when missing, so a changed kernel config is otherwise ignored).

## Root filesystem: write the other slot, then switch the kernel

The stage-2 installer (RAM-booted kernel that writes root0) did NOT start through this window on
2026-11-26: after the detach the board reset straight into the installed system. Not understood.
What works instead, with the running system as the fallback:

1. Stream the jffs2 into the inactive root partition and verify it:

       cat j49-image.jffs2 | ssh root@<ip> 'flash_erase -q /dev/mtd8 0 0 && nandwrite -q -p /dev/mtd8 -'
       ssh root@<ip> 'nanddump -q --bb=skipbad -l <len padded to 2048> /dev/mtd8 | sha256sum'

   Compare with the sha256 of the image padded with 0xFF to a multiple of 2048.
2. Build a kernel whose built-in command line roots on that partition
   (`CONFIG_CMDLINE="root=/dev/mtdblock8 ..."`, the unmodified production DTB) and write it to
   boot0 as above. The slot is chosen by the kernel's built-in command line, not by U-Boot.

The system on root0 (mtd6) stays intact; going back is one boot0 write with the A-slot uImage
(`installer/out/uImage-j49-a`).

## Which slot can be written from which system

The partition table in the DTB decides it. Under the B kernel (`uImage-j49-b`, production DTB)
root0 is read-only (`/sys/class/mtd/mtd6/flags` = 0x0), so a system running from root1 cannot
write root0; under the A kernel root0 and root1 are both writable. Seen on 2026-11-26. Going from
B to a new image in the other slot therefore needs two boot0 writes over USB (A kernel, write
root1 from the root0 system, B kernel), or a B kernel built with root0 writable.

## Without USB: only the files that differ

Done on 2026-11-26 for 701eccd3, with the device on the backplate. When the kernel stays the same
the image can be brought onto the running root filesystem file by file:

1. On the PC, hash every regular file in `j49-image-nest-j49.rootfs.tar.xz`; on the device
   `find / -xdev -type f -exec sha256sum {} +`. Compare, also the symlinks and modes.
2. Leave what belongs to this unit: `/etc/machine-id`, the WL1271 NVS, the WiFi file, the SSH host
   key, everything under `/var/lib`.
3. Put the files that differ in a tar (same owner and mode as in the image), unpack it in `/run`,
   check with the loader that every program finds its libraries, then `cp` to `<name>.new` and
   `mv` over the old file, `sync`, and check the hashes on the device.
4. Restart the services, then reboot, then compare all files once more.

That day 7 of 1012 files differed (the release file, the four j49 programs, two systemd presets).
Do not "test" a daemon with `--version`: `j49-mqtt` and `j49-screen` take no arguments and simply
start, a second copy that never exits.

## Files that survive

`j49-wlan-reprobe` copies from the scratch partition (mtd13) at boot:
`j49-persist/rootfs/` when the target is missing (WiFi credentials, SSH host key) and
`j49-persist/override/` when it differs (this unit's WL1271 NVS, i.e. its MAC address).
