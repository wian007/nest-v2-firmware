# Updates over the network (j49-update)

**State (27 November 2026): written and tested on a PC against stand-ins for the flash tools.
Not yet run on a thermostat, and not yet part of the image.**

The thermostat can take a new root file system over WiFi, without USB and without the ring
gesture. Nothing happens by itself: looking for a newer version is off unless the owner
switches it on, and installing is always an order.

## How it works

The flash memory has two places for a system, `root0` and `root1`. An update never touches the
one that runs.

1. `j49-update` streams the image into the other slot (it is never held in RAM), compares its
   SHA-256 with the one that was announced, and reads the slot back. When either differs, the
   slot is erased again and nothing else changes.
2. What belongs to this thermostat (WiFi networks, settings, SSH keys) is copied to the
   `scratch` partition, from where every system takes it at its first start.
3. A record in the partition `env1` asks for a **trial** of the new slot. The thermostat
   restarts.
4. `nandio`, the first program the kernel starts, counts the trial down and starts the new
   system. When that system is up for two minutes, `j49-update confirm` (a systemd timer) makes
   it the active one.
5. When the new system does not come up, the watchdog restarts the board. The trial is used up,
   and `nandio` starts the old system again.

The record is one line of text: `J49SLOT1 active=<0|1> try=<0|1|-> left=<n> sum=<crc32>`.
Without a record the kernel command line decides, as before.

**The kernel is not part of this.** It sits in `boot0`, has no second copy, and is updated over
USB ([updating](../tools/dfu-install/update-from-linux.md)). A manifest can name the kernel it
needs; `j49-update` then refuses instead of installing a system that cannot run.

## Settings and commands

`/var/lib/j49/update`:

    check=off        # on: ask the address below once a day whether there is a newer version
    url=https://.../manifest.txt

The manifest, next to the image on any https server:

    version=2026.12.01
    rootfs=j49-image-nest-j49.rootfs.jffs2
    size=21548920
    sha256=<hex>
    kernel=6.6.64          # optional

| Command | What |
|---|---|
| `j49-update status` | running slot, the record, what was found |
| `j49-update check --now` | fetch the manifest once, whatever `check=` says |
| `j49-update install` | install what the manifest names |
| `j49-update install <file or https address> <sha256>` | install this image |
| `j49-update confirm` | make the running slot the active one (the timer does this) |
| `j49-update back` | ask for a trial of the other slot as it is: back to the system before the update |

`/run/j49-update.state` holds `installed=`, `latest=`, `checked=`, `busy=`, `error=` for the web
page and Home Assistant.

## What it does not do yet

- The image is checked against the SHA-256 in the manifest. Manifest and image come from the
  same server: there is no signature yet, so whoever controls that address controls the update.
- No button on the web page and no update entity in Home Assistant yet.
