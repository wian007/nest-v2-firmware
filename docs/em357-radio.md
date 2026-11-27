# The 802.15.4 radio of the Nest J49 (Silicon Labs EM357)

Notes of 2026-11-26. "Observed" = seen on the device or in a file; "inferred" = concluded from
that; "unknown" = not established.

## Hardware and wiring

- EM357: Cortex-M3, 192 kB flash, 12 kB RAM, 2.4 GHz IEEE 802.15.4 (data short, Silicon Labs 2012).
- Observed: connected to the OMAP over SPI, McSPI2 CS0 (our `/dev/spidev1.0`, stock
  `/dev/spidev2.0`). Reset# = GPIO 61, host interrupt = GPIO 182 (falling edge). Stock's
  `diamond-zigbee` also has `wake` and `power_enable`; their GPIO numbers are unknown.
- Observed: stock forces the chip into its bootloader by muxing the SPI chip-select pad
  (mcspi2_cs0, pad 0x21dc) to GPIO 181, driving it low and pulsing reset (`/etc/init.d/em357`).
- Unknown: whether the EM357 UART pins (PB1 TXD, PB2 RXD) reach the OMAP. In SPI NCP designs PB2
  is the host-interrupt line, so a UART firmware most likely cannot be used on this board.
- Observed in stock `wpantund.conf`: `TXPowerMode 5` ("boost mode, bidirectional antenna port,
  external PA present"), `TXPower -15`, `NCPTXPowerLimit -10` ("to protect the WiFi FEM"). The
  board has an external front end; any other firmware must drive it the same way.

## Firmware on the chip today

- Observed: `ConnectIP/2.0.4b301 s4 m7 (Alpha); Sep 23 2015 16:39:22`. Silicon Labs' pre-Thread IP
  stack with Nest's "Legacy" network. Not spinel/OpenThread, not ZigBee.
- Observed: stock `spi-server` + `wpantund` in the stock chroot on our kernel bring it up
  (`tools/thread-poc/runwpan.sh`): reset ok, state `associated`, TUN interface `wpan0`.
- `usr/share/silabs/data/firmware/ip-modem-app.bin` (131980 bytes, sha256 19a19640...) is that
  application. Observed: it is a plain, unencrypted Cortex-M3 image with a standard Ember
  application address table at the start: stack pointer 0x20001ad8, reset vector 0x08020869,
  table type 0x0AA7, vector table 0x08002100, plat/micro/phy = 4/3/3 (Cortex-M3 / EM357),
  software version 0x2044 build 0x012d (= 2.0.4 b301). Inferred: it is linked at 0x08002000, the
  application area behind an 8 kB bootloader. `deep-sleep-app.bin` (5480 bytes) has the same header.

## Nest's SPI bootloader ("em357blo-1.0")

Decoded from stock `/nestlabs/bin/zb-loader` ("nlbl-em35x-1.04") and then exercised on the
device with our own read-only tool `tools/j49-ncp` (2026-11-26). Everything in this section is
observed unless marked otherwise.

**Entering it** (stock `/etc/init.d/em357 bootloader`, `j49-ncp enter`): mux the SPI chip-select
pad (mcspi2_cs0, pad register 0x480021dc) to GPIO 181, drive it low, pulse reset# (GPIO 61) low
for 1 s, wait 1 s, mux the pad back. `j49-ncp reset` (reset pulse with the pad left alone) returns
to the application; afterwards wpantund reported ConnectIP running and `associated` again.

**SPI**: mode 0, 8 bits, 500 kHz. The answer is fetched in a second transfer of fill bytes (0xEE).

**Request** (little endian): `FF FE | u16 len (= total - 4) | u16 class | u16 cmd | u32 unused |
payload | u32 0xCAFE1A7E | FF`. **Answer**: `FF FE | u16 len | u16 class|0x8000 | u16 cmd | u32 |
result at offset 12 ...`.

| request | class, cmd | payload | wait | answer |
|---------|-----------|---------|------|--------|
| version | 0, 0xFFFF | none | 2 ms | 33 bytes, string at offset 12: `em357blo-1.0` |
| erase (not used by us; from the disassembly) | 2, 2 | addr, size | 2 s | 21 bytes |
| write (not used by us; from the disassembly) | 2, 3 | addr, size (<= 64), data[64] | host interrupt | 21 bytes |
| read one word | 2, 6 | 0, addr, 4 | 2 ms | 21 bytes, the word at offset 12 |
| CRC of a range | 2, 7 | addr, size | host interrupt | 21 bytes, CRC at offset 12 |

The CRC is the bitwise inverse of the usual CRC-32 (zlib): chip `0x02308469` for the application
area, zlib over the same bytes `0xfdcf7b96`.

**Flash contents, dumped with `j49-ncp read`** (4 bytes per request, about 2 minutes for all):

- `0x08000000..0x08001FFF` bootloader, 8 kB, contains the string `em357blo-1.0`.
- `0x08002000..` application: byte for byte identical to stock's `ip-modem-app.bin` (131980 bytes).
- behind it up to `0x0802FFFF`: mostly erased, 1130 non-FF bytes (token/NVM area; the application
  address table gives 0x0802E000 as its start).
- `0x08040000..0x08040FFF` info blocks: the EUI64 sits at offset 0x812.

The whole-flash CRC reported by the chip matches the dump. The dump is kept with the device
backup, not in this repository: it holds the stored
network credentials.

Inferred, not tried: since the bootloader writes by address and the application image is a plain
binary at 0x08002000, another EM357 application linked for that address can be loaded, and the
original put back from the dump, as long as the first 8 kB are never written.

## ZigBee firmware: what exists and what stands in the way

- `github.com/grobasoz/zigbee-firmware` has EmberZNet 6.7.8 / 6.7.10 NCP builds for the EM357
  (EZSP v8, works with zigpy/ZHA and Zigbee2MQTT according to its issue #47). All EM357 builds
  there are UART (PB1/PB2); an SPI NCP exists there only for the EM3581, a different chip.
- Their `.ebl`/`.s37` files target the standard Ember bootloader. Nest's bootloader takes a raw
  `.bin` linked at 0x08002000, so an image would have to be converted, and it must not contain or
  overwrite the first 8 kB.
- ZHA and Zigbee2MQTT speak EZSP over a serial line or TCP, not over SPI: a bridge on the device
  would be needed.
- ZigBee firmware replaces ConnectIP, i.e. no link to a Heat Link while it is installed.

## An EZSP SPI image for the EM357 does exist (found 2026-11-26, not flashed)

`github.com/yqyunjie/Zigbee-Project` (a public mirror of an EmberZNet 5.0.0 tree from 2013, no
licence stated; Silicon Labs' SDK is proprietary, so treat the files as "available", not as
"licensed") has prebuilt NCP images under
`firmware/EmberZNet/EM35x-EZSP/build/em35x-ezsp-images/EM357/`, among them
`em357-ncp-spi-use-with-ezsp-spi-bl-500.s37` (sha256 a0860607...) and `.ebl`.

Observed by converting the `.s37` to a binary:

- address range 0x08002000..0x0801DB38 (113464 bytes): the same base as Nest's application;
- the same application address table as `ip-modem-app.bin`: type 0x0AA7, vector table
  0x08002100, plat/micro/phy 4/3/3 (EM357), table size 0xAC; software version 0x5000 build 95;
- its non-volatile (SimEEPROM) area starts at 0x08027000, Nest's at 0x0802E000: loading it will
  overwrite the stored ConnectIP network data (we have the dump);
- strings name `spi-protocol.c`, i.e. the EZSP-SPI host interface.

Inferred: Nest's bootloader can load it as a plain binary at 0x08002000. Unknown until tried:
whether its pin use (nHOST_INT, nWAKE, front-end control) matches this board, and whether
EmberZNet 5.0.0 (EZSP protocol version 4, ZigBee PRO from 2013, no Zigbee 3.0) is new enough for
the host software. bellows/ZHA lists EZSP v4 as its oldest supported version; Zigbee2MQTT needs
newer firmware. A 6.7.x SPI build would still be the better target.

## Next steps

1. Done: protocol decoded, bootloader entered, full dump taken and verified.
2. Found: an EZSP SPI NCP image for the EM357, EmberZNet 5.0.0 (see above). Open: a 6.7.x one.
3. Open: a host-side bridge from EZSP-SPI to a TCP/serial port for ZHA or Zigbee2MQTT.
4. Before any write: add erase/write/verify to `j49-ncp`, restricted to 0x08002000 and above.
