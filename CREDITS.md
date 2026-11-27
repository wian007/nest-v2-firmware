# Credits

This project stands on the work of others. Thank you.

## The way into the thermostat

- **GTVHacker** (<https://github.com/gtvhacker/NestDFUAttack>, 2014): found that the boot ROM of
  the thermostat accepts unsigned code over USB, and published the loaders and the rescue
  kernel that every later project uses. The first step of our installer is their attack.
- **cuckoo-nest** (<https://github.com/cuckoo-nest>): keeps the sources of the x-loader, U-Boot
  and the 2.6.37 kernel buildable (`cuckoo_loader`), and runs the wiki with the research on the
  backplate and the Heat Link. Our service kernel is built from those sources.
- **Grant Hernandez** and **ajb142** (<https://github.com/ajb142/omap_loader>): `omap_loader`,
  the tool that talks to the OMAP boot ROM and the x-loader. `tools/omap-loader` is their
  program (GPL-2.0); `tools/web-installer/js/omap.js` follows its protocol.
- **Texas Instruments**: the x-loader and the documentation of the OMAP3 boot ROM.

## What it is built with

- The **Linux kernel**, **U-Boot**, the **Yocto Project** and **OpenEmbedded**, and the many
  projects in the image built with them. Each keeps its own licence.
- **BusyBox** and the **GNU C Library** in the service kernel.
- **Rust** and its standard library for our own programs.

## Fonts

See [tools/themes/fonts/LICENSES.md](tools/themes/fonts/LICENSES.md): Roboto (Google), Inter
(The Inter Project Authors), Noto Sans (The Noto Project Authors), Matrix Sans (The Matrix Sans
Project Authors), and the X Window System "misc-fixed" font in the distribution of Markus Kuhn.

## Ideas and formats

- **Home Assistant**: the look of the thermostat card that the dial follows, MQTT discovery,
  and the colours of the two Home Assistant themes.
- The melodies that are not our own are old tunes in the public domain, written out by us in
  RTTTL (the ring tone format of Nokia).

## Not affiliated

None of the people and projects above is responsible for this one, and this project is not
affiliated with Google or Nest. See the [disclaimer](DISCLAIMER.md).

If your work is used here and not named, or named wrongly: please open an issue.
