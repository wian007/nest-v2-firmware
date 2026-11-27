# Disclaimer

Read this before you install anything from this repository on a thermostat.

## No affiliation

This is an independent hobby project. It is not affiliated with, endorsed by, sponsored by or
supported by Google LLC, Nest Labs, or any maker of heating equipment. "Nest", "Nest Learning
Thermostat" and "Heat Link" are trademarks of their owners and are used here only to say which
hardware the software runs on.

## It controls real heating

The software switches a heating system on and off. A fault in it, in your installation, or a
thermostat that runs out of power can leave the heating **off** (frozen pipes, a cold house) or
**on** (overheating, wasted energy, damage). The stock firmware has safety functions that this
software does not have, or has not shown to work. Do not rely on it where a failure can cause
harm, and do not leave it unattended until you have watched it work in your own installation,
including what happens when it loses power or its network.

Known at the time of writing: on a real backplate the thermostat's battery can drain when the
head unit does not sleep enough, and the head unit then switches off. See `docs/sleep-j49.md`.

## It replaces the firmware of your thermostat

Installing overwrites the stock firmware. After that the thermostat no longer works with the
Nest or Google Home apps and services, and it no longer receives updates from its maker. You
will most likely lose any warranty. Make the full backup described in the installation guide
first; without it there may be no way back. An interrupted or failed installation can leave the
device unusable.

## Mains voltage

A Heat Link and a boiler are connected to mains voltage. This project does not ask you to open
or rewire them. If you do work on the wiring, switch off the power first and have a qualified
installer do what the rules where you live require.

## No warranty

The software and the documentation are provided "as is", without warranty of any kind, as stated
in the [LICENSE](LICENSE). You use them at your own risk. The authors are not liable for damage
to your thermostat, your heating system, your home, or anything else.

## Other people's software

Files that come from other projects keep their own licences: the Linux kernel and U-Boot
(GPL-2.0), the packages in the image built with Yocto, and so on. This repository contains our
patches and build recipes for them, not their source trees. Firmware built by the maker of the
thermostat is not in this repository.

## Reverse engineering

The protocol descriptions in `docs/` were worked out by studying a device we own, in order to
make our own software work with its hardware. They describe interfaces; they do not contain the
maker's code.
