# Nest J49 ("diamond") hardware reference

Raw dumps captured from the live stock device (firmware 5.9.4-5, kernel 2.6.37)
via root SSH, 2026-10-25. Used to write the mainline device tree and defconfig.

- `vendor-kernel-2.6.37.config` — stock kernel config (from /proc/config.gz)
- `omap_mux.txt` — full OMAP3 pinmux (202 pads, /sys/kernel/debug/omap_mux)
- `gpio.txt` — GPIO allocations with driver labels
- `clock.txt` — clock tree
- `pm_voltage.txt` — power domains / voltage (OPP)

## SoC / boot
- TI OMAP3, **GP (General Purpose) device** — confirmed by x-loader string
  "TI OMAP SPL With GP CH". GP => ROM accepts **unsigned** boot images over the
  USB/UART peripheral boot path. This is the "DFU"/recovery route and makes the
  board effectively unbrickable (mask ROM always offers USB boot).
- Cortex-A8, likely Sitara AM3703; 64 MB RAM @ 0x80000000; loadaddr 0x81000000.
- Console ttyO0 @ 115200. Stock U-Boot 2012.10 "diamond".

## Named GPIOs (for the device tree)
| GPIO | Label | Dir | Notes |
|------|-------|-----|-------|
| 42  | DIAMOND_GPIO_INTUSB | out | USB interrupt/VBUS |
| 58  | PIEZO_N | out | piezo buzzer |
| 61  | ZigBee Reset# | out | Ember EM357 reset |
| 65  | Backplate Detect | in | irq, edge-both |
| 101 | ADBS A330 Shutdown | out | Avago optical ring sensor |
| 102 | WL127x IRQ | in | WiFi irq, level-high, wakeup |
| 103 | vmmc2 | out | SDIO (wl12xx) power |
| 104 | Backplate Reset | out | |
| 105 | s6d05a1 lcd_id | in | panel id |
| 108 | ADBS A330 Motion# | in | irq, edge-falling, wakeup |
| 109 | s6d05a1 reset | out | LCD panel reset |
| 110 | ADBS A330 Reset# | out | |
| 161 | Battery Disconnect | out | |
| 177 | lm3530 enable | out | TI LM3530 backlight/ALS |
| 182 | ZigBee Interrupt | in | EM357 irq, edge-falling |

## Peripherals → mainline driver mapping
| Function | Part | Bus | Mainline driver |
|----------|------|-----|-----------------|
| WiFi | TI WL1271 (WiLink6) | SDIO (vmmc2) | wlcore / wl12xx (+ wl127x fw, NVS) |
| Display | Samsung S6D05A1 / Tianma TM025ZDZ01 | SPI (9-bit) + OMAP DSS | needs panel driver (port from vendor / panel-mipi-dbi) |
| Backlight/ALS | TI LM3530 | I2C | leds-lm3530 / backlight |
| Optical ring | Avago ADBS-A330 | SPI | custom / input driver |
| ZigBee (Weave) | Ember/SiLabs EM357 | UART or SPI (NCP) | out-of-tree (not Linux 802.15.4; CONFIG_IEEE802154 off) |
| Backplate | STM32L151 Cortex-M3 | serial | talks to head unit; relays/HVAC + SHT20 |
| NAND | 256 MB | GPMC | MTD_NAND_OMAP2 |

Vendor driver sources/behaviour for the S6D05A1 panel and ADBS ring can be
reverse-engineered from the vendor 2.6.37 kernel (mtd6_boot0.bin in the backup).

## Bluetooth: not present (investigated 2026-11-20)

The J49 has no usable Bluetooth. Do not spend time on it again unless the
schematic or the PCB shows otherwise.

Evidence from the stock firmware (our backup of 2026-10-25, root0 +
vendor config):

- Vendor kernel: `# CONFIG_BT is not set`. Only `WL12XX`/`WL1271_SDIO` are on.
- `lib/modules/2.6.37` holds only cfg80211, mac80211, compat, wl12xx,
  wl12xx_sdio, firmware_class. No hci_uart, no ti-st.
- `lib/firmware/ti-connectivity/` has only `wl127x-fw-4-{sr,mr,plt}.bin` and
  `wl1271-nvs.bin`. No `TIInit_*.bts`. No `hciattach`, no bluez.
- `tiwlan.ini` has no BT coexistence settings.
- BLE strings exist in `nlclient` and `/etc/init.d/nestlabs` ("BLE Dialog chip",
  `ble.sh`), but only for the models `Flintstone-1.*` and `Display-4.*`. On J49
  `ble_actions` is empty and `ble.sh` does not exist. It is shared Nest code.

Evidence from the pinmux dump (`omap_mux.txt`):

- A Bluetooth HCI link to the WL1271 needs a UART with RTS/CTS. The only
  candidate is UART2 on the `mcbsp3_*` pins (balls W4, V5, V6, V4). All four are
  in `safe_mode`, so stock did not use them.
- UART1 = debug console, UART3 TX/RX = backplate (`ttyS2`, no RTS/CTS wired).
- No `BT_EN` in the GPIO list.

Caveat: the WL1271 silicon can do Bluetooth. The dump only shows what stock
enabled. The one remaining check is whether balls W4/V5/V6/V4 go anywhere on
the PCB.
