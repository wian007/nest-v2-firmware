# Contributing

Contributions are welcome: fixes, a language, a theme or a font, a finding about the hardware,
a test report from your own thermostat.

## Before you start

- Read the [disclaimer](DISCLAIMER.md). The software switches real heating: say in a pull
  request what you tested and on what (a PC, the emulator, a thermostat), and what you did not
  test. "Not tested on a device" is a fine thing to write; leaving it out is not.
- For anything larger than a fix, open an issue first, so nobody does work twice.
- Only the UK/EU backplate with a Heat Link is supported, for heating only. The US backplates
  and the other HVAC circuits are on the roadmap in the [README](README.md#roadmap); a test
  report or a capture from such a backplate is welcome.

## Easy ways in

- **A language:** copy `tools/lang/en.txt` to `<code>.txt` and translate the texts after the
  `=`. The same for the installer page in `tools/web-installer/lang/` (and a flag in
  `js/app.js`). The screen has capitals, digits and a few signs unless the theme uses a font
  with more.
- **A theme or a font:** `tools/themes/`, explained in
  [docs/thermostat.md](docs/thermostat.md#languages-locale-and-themes). Say where a font comes
  from and under what licence (`tools/themes/fonts/LICENSES.md`).
- **Try things without a thermostat:** the [emulator](https://github.com/wian007/j49-emulator)
  runs the screen program on a PC.

## Rules of the house

- Everything in the repository is in English: code, comments, documents, commit messages.
- Our own programs are small Rust programs without dependencies. They talk to each other
  through text files (`docs/thermostat.md`), not through each other's code. Keep it that way.
- Heating is off until the user switches it on. Sleeping is on unless the user switches it
  off. A change must not alter these defaults.
- No secrets, addresses of your own network, serial numbers or backups in a commit.
  `tools/release/export-public.sh` checks for the common ones.
- Files built by the maker of the thermostat do not go into the repository.
- Unit tests run on a PC: `cargo test --release --target x86_64-unknown-linux-gnu` in the
  directory of a program; `python3 tools/dfu-install/nandio/test.py` and
  `node tools/web-installer/test/run.mjs tools/dfu-install/nandio/nandio-host` for the installer.

## Licence of contributions

By contributing you agree that your contribution is under the [MIT licence](LICENSE) of the
project, or, for a patch to the Linux kernel or U-Boot, under the licence of that project.

People and projects this one builds on are in [CREDITS.md](CREDITS.md).
