# Updates over the network (j49-update)

**State (27 November 2026): written and tested on a PC, with the real releases on GitHub and
stand-ins for the flash tools. Not yet run on a thermostat.**

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
USB ([updating](../tools/dfu-install/update-from-linux.md)). The reason: U-Boot reads `boot0`,
and U-Boot and our kernel use different ECC layouts, so what our kernel writes there U-Boot
cannot read ([flash-layout.md](flash-layout.md)). `boot0` must be written by U-Boot. A release whose system needs a newer
kernel cannot say so yet.

## Versions and repositories

A version is a **release of a git repository** (GitHub, or Gitea/Forgejo, which list releases in
the same way). A release that can be installed has two files attached: the root file system
(`*.rootfs.jffs2`) and `SHA256SUMS` with its checksum.

- The firmware carries a list of **standard repositories**: `/usr/share/j49/update-repos`, one
  https address per line. The public image names `github.com/wian007/nest-v2-firmware`; an image
  of your own replaces the file.
- The owner can name **repositories of their own** next to them (any number, each with a token
  when it is not public: a personal access token that can only read). Standard and own are
  used side by side; `standard=off` leaves the standard ones out.
- Every version is shown with the repository it comes from, and whether that is a standard one
  or one of your own. What was installed remembers its repository. When two repositories have a
  version of the same name, `install` wants to be told which (`--from=<address>`); the web page
  always tells.

## Settings and commands

`/var/lib/j49/update` (readable for root only: it can hold a token):

    check=off          # on: look for a newer version once a day
    prerelease=off     # on: count pre-releases too
    standard=on        # off: only the repositories below
    repo=https://git.example.org/me/my-firmware <token>

| Command | What |
|---|---|
| `j49-update status` | version, commit and commit date of what runs, the slot, the repositories |
| `j49-update list` | the versions the repositories offer |
| `j49-update check --now` | the same, and note the newest (the daily timer runs `check`, which does nothing unless `check=on`) |
| `j49-update install` | install the newest version |
| `j49-update install <version> [--from=<repository>]` | install that version (an older one too) |
| `j49-update install <file or https address> <sha256>` | install this image |
| `j49-update repo add <address> [token]` / `repo remove <address>` / `repo reset` | repositories of your own |
| `j49-update confirm` | make the running slot the active one (the timer does this) |
| `j49-update back` | ask for a trial of the other slot as it is: back to the system before the update |

"Newer" means: another name than what runs, and published after this system was built.

The web page has the same under "Updates over the network": what runs now, the daily look on or
off, test versions on or off, the list of versions with a button per version, and the
repositories.

`/run/j49-update.state` holds `installed=`, `commit=`, `latest=`, `newer=`, `checked=`, `busy=`,
`error=`; `/run/j49-update.versions` the list as it was last fetched.

## What it does not do yet

- The image is checked against `SHA256SUMS` of the same release: there is no signature yet, so
  whoever controls the repository controls the update.
- No update entity in Home Assistant yet.
- No update of the kernel (see above).
- It was not run on a thermostat yet.
