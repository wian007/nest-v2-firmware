# How stock nlclient drives the backplate (and through it the heating)

Static analysis of `/nestlabs/sbin/nlclient` (stock 5.9.x, build Sep 12 2019), 2026-11-26. Method:
the binary was disassembled, every call to the backplate send function (0x1c54d4) was listed with
its command byte, and the log strings around senders and receive handlers were read. Labels:
**observed** = in the binary or in our own serial captures; **inferred** = concluded from that;
**unknown** = not established. The first test against a Heat Link is in its own section below.

Frame format and the sensor side are in `backplate-protocol.md`.

## The two controllers

nlclient has two separate pieces of code (observed):

1. **Backplate controller** (code 0x1c3000..0x1cd800, log prefix `backplate controller:`). It owns
   the serial line: reset, wires, pins, switches (the relays/FETs), power stealing, sensor
   buffers, safety temperature, backplate firmware update.
2. **Heat Link controller** (code 0x1d6000..0x1db000, log prefix `heatlink controller:`). A state
   machine ("Handling event %s (current state %s)") that sets switches and a manual mode on a
   *remote* Heat Link and reads boiler information (`RemoteModulatingBoilerInfo/Diag/Counters`,
   OpenTherm register logs). It does not call the backplate send function. Inferred: this is the
   **wireless** (Weave over the 802.15.4 radio) path, including everything OpenTherm.

The user interface knows three ways to be connected (observed strings): `wiredonly`,
`wirelessly`, `wiredandwirelessly`, and a state "not wired nor wireless". The backplate glue logs
`connection states - bp: %d hl: %d both: %d` and can update Heat Link firmware "via the
backplate" (`backplate: Heat Link update succeeded`).

Inferred for our hardware (Heat Link without OpenTherm: N, L, relay 1/2/3, T1 = ground,
T2 = 12 V): with only T1/T2 connected, calling for heat is a **backplate switch command**; the
backplate signals the Heat Link over the two wires. The message exchange on those two wires is
unknown and is the backplate's business, not the head unit's.

## Commands the head unit sends (observed command bytes)

`send(this, cmd, buffer, frame_length, flag)`: the length argument is the whole frame (3 sync +
2 type + 2 length + payload + 2 CRC), so "11" means a 2-byte payload.

| cmd | what nlclient says when it sends it | payload |
|-----|--------------------------------------|---------|
| 0xff | "sending reset, waiting for wire state response" | none |
| 0x8f | "sending wire ACK" | the 13 bytes of the wire state it received, echoed |
| 0x82 | "sent circuit change request for switch id (%d) with state (%d)" | 2 bytes: circuit 0..12, state |
| 0xc0 | "setting power steal mode to 0x%08x" | u32 mask |
| 0xa1 | "sending quiet message (max_sleep_seconds: %u)" | u16 |
| 0xa4 | "sending set wakeup mask %04x" | u16 (bits in `backplate-protocol.md`) |
| 0xa2 | "sending flush buffer message" | none |
| 0xa3 | "end of buffers ACK" | none |
| 0x8b | "Setting the MCU mode to %d" | |
| 0xc3 | "setting temp comp mode to %u" | |
| 0xb9 | (near) "disabling high als threshold" | |
| 0xc4 / 0xc5 / 0xc6 | "sending MSG_GET_ERRORS" / "..._OC_ERRORS" / "..._TP_ERRORS" | none |
| 0x91 / 0x92 / 0x93 | "sending begin update request" / "requesting firmware update record" / "sending end update request" | backplate firmware update |
| 0x90, 0x98, 0x99, 0x9b..0x9f | identity queries (firmware id/version/info, BSL id/version/info, serial, model) | none |
| 0x83, 0x85, 0x86, 0x8a, 0x8c, 0xb1, 0xb3, 0xb5..0xb8, 0xba, 0xc2 | sent, purpose not read from the code yet | |

This corrects two names in `backplate-protocol.md`: the 13-byte message `0x0004` of the reset
burst is the **wire state** (`MSG_WIRES`), and `0x8f` is the **wire ACK**. Not "FET presence".

## Messages the backplate sends (observed handler names)

`MSG_WIRES`, `MSG_PINS`, `MSG_ACK_SWITCH`, `MSG_BP_STATUS`, `MSG_POWER` ("got power message detect:
%d wire: %d current: %dmA"), `MSG_PWR`, `MSG_ERRORS`, `MSG_OC_ERRORS` (over-current, with the
wire state), `MSG_TP_ERRORS`, `MSG_ST_STATUS` ("rcvd safety temp data (status, current, heattime,
cooltime, reason, ocevents, pwrevents)"), `MSG_IPS_STATUS`, `MSG_WAKEUP`, `MSG_WAKEUP_VEC`,
`MSG_UPDATE_ACK`, `MSG_FW_ID`, `MSG_FW_ID2`, `MSG_BSL_ID`, `MSG_END_BUF`, and the sensor family
`MSG_TEMP`, `MSG_TEMP2/3`, `MSG_PIR`, `MSG_NEAR_PIR`, `MSG_PROX`, `MSG_ALS`, `MSG_SENSOR`, `MSG_TP`,
`MSG_PIR_DATA_2`, each also as `..._BUF` (history).

The dispatch goes through an internal event number, not the wire type, so the type codes are not
read from the code yet. Matched by payload length against our captures (inferred): `0x0004` =
WIRES (13), `0x0009` = PINS (15, "pin state: 15 values"), `0x000a` = ALS (4), `0x000c` = SENSOR
(14), `0x0010` = FW_ID (3), `0x0038` = PIR_DATA_2 (35), `0x002f` = END_BUF. `MSG_ACK_SWITCH` = `0x0006` (from the backplate firmware). Unknown: what `0x0005`, `0x0007`, `0x000b`, `0x0014`, `0x0017` are (the 1 Hz
`0x0007` that we show as "motion" may be NEAR_PIR or PIR).

## The flow

**Start-up** (observed in the code and in our capture of 2026-11-24):

1. `0xff` reset; the controller then waits for the wire state.
2. Backplate: version text, `MSG_WIRES` (13 bytes, one per wire/circuit), `MSG_PINS` (15), status
   text, "BRK".
3. Head unit: `0x8f` wire ACK with the same 13 bytes. Without the echo the backplate does not
   start its sensors (found by experiment).
4. Identity queries; `0xc0` power-steal mode; `0xa2`/`0xa3` to drain the sensor history.
5. From the wire state nlclient works out which equipment exists: it maps logical switches
   (`nlSwitchStageOneHeat`, `...StageTwoHeat`, `...StageOneCool`, `...Fan`, `...OB`,
   `...AuxiliaryHeat`, `...EmergencyHeat`, `...Humidifier`, `...Dehumidifier`, `...WaterHeater`,
   ...) to **circuits**; "failed to get circuit for nlSwitchX" when the wire is absent. Wire codes
   in the binary: W1, W2_AUX, Y1, Y2, G, OB, RC, RH, C, STAR.

**Calling for heat** (observed in the function at 0x1c558c):

1. The thermostat logic asks for a switch change: "Heater (%u)->(%u)".
2. The controller sends `0x82` with two bytes, `[circuit][state]`, logs "sent circuit change
   request for switch id (%d) with state (%d)" and marks that circuit as waiting.
3. The backplate answers `MSG_ACK_SWITCH`: "got ACK_SWITCH for switch id (%d) with state (%d)".
   Other outcomes are logged as "unexpected state", "invalid switch id", "unexpected ACK_SWITCH".
4. Stopping is the same request with the other state. On a reset or disconnect: "Clearing all
   switches."

**While running** (observed strings):

- A periodic tick; "SendTick about to reset: now ... (connected: %d)" resets the backplate when
  it stops answering.
- "HU and BP power state mismatch hvac_power: %d pwr_state: %d, resetting the BP" (with a retry
  limit): the head unit compares what it asked for with what the backplate reports.
- Power stealing is restricted while a circuit is switched (`MSG_DISABLE_PS_WHILE_SWITCHED`, a
  circuit bitmask; skipped on MCU models that do not support it).
- Errors: `MSG_ERRORS`, over-current per wire ("wire %d (switch %d) has an error but its state is
  off"), "Error condition(s) cleared due to switch(es) off".
- Safety temperature: "SetSafetyTempSettings Enable: 0x%02X Heat: %d-%d Cir: 0x%04X Cool: %d-%d
  Cir: 0x%04X lockout: %d". Inferred: the backplate is given limits and circuits so it can switch
  heating by itself when the head unit is asleep or dead.
- Before sleeping: `0xa1` (maximum sleep time) and `0xa4` (wake mask).

## What the backplate firmware does with it (AMBER_BP 1.0.26, disassembly, 2026-11-26)

Observed in the firmware image (`nlbpfirmware.plist`, load address 0x08000000):

- **Handler of 0x82** (0x08001828): accepts the command only when the flag at 0x20000440 is 1 and
  the payload is exactly 2 bytes; circuit = byte 0 (must be <= 12), state = byte 1. It calls the
  switch function, reads the resulting state back and answers with **message type 0x0006**,
  payload `[circuit][state now]`. So `MSG_ACK_SWITCH` = 0x0006.
- **The flag at 0x20000440** is set to 1 in the handler of `0x8f` (wire ACK, 0x080018e4) and
  cleared on reset. A switch command before the wire ACK is ignored.
- **The switch function** (0x08005670) returns at once for every circuit except **0**. This
  backplate has exactly one switch. State 0 = off, anything else = on; the current state is the
  byte at 0x20000bf0. Asking for the state it already has does nothing.
- **Switching** reprograms a timer (TIM3 at 0x40000400, output on port A pin 6) and a DAC channel
  (0x40007408): a continuous pulse train. Off and on differ in the **period** of that train
  (two stored values), and the DAC selects between two current levels.
- **Defaults** set at start-up (0x08005834): `timing(60000 ms, 10 ms, 50 ms)` and
  `current(100 mA, 8 mA)`; the debug log prints them as "period %u ms, pulse %u ms, guard %u ms"
  and "mA: %u %u, dac: %u %u". The period used for "on" is computed from a constant 1000 ms.
- Strings in the same module: "wire scan", "power scan", "%d devices found", "*HVAC power up",
  "*HVAC power lost", "power source", "limit %d mA".

Inferred: over T1/T2 the backplate signals the Heat Link with **current pulses on the 12 V
supply**: one 10 ms pulse per second while heat is called for, one per 60 s otherwise. That would
also make it fail-safe (no pulses, no heat). The meaning of the two currents and of the 50 ms
"guard" is unknown, and none of this has been measured on the wires.

So for the Heat Link the command is: **`0x82 00 01` = heat on, `0x82 00 00` = heat off**, valid
after the wire ACK, answered by `0x0006 00 <state>`.

## Tried on the test backplate with the Heat Link (2026-11-26)

Observed with our own `bpd` (no stock software running):

- `0x82 00 01` after the wire ACK is answered at once with `0x0006 00 01`, and `0x82 00 00` with
  `0x0006 00 00`. Every switch command of that evening was acknowledged on the first try.
- The Heat Link relay clicks audibly after "on" (reported by the user at the device). The click
  comes **later** than the acknowledgement: `0x0006` only says the backplate took the command, not
  that the Heat Link switched. How much later was not timed; it fits the inferred signalling of
  one pulse per second.

So the command derived from the firmware is right. Still unknown: the delay in seconds, whether
"off" is as fast, and any feedback from the Heat Link itself (none is known in the protocol).

## What a minimal controller of our own needs

Inferred from the above; the order to try it on the test backplate with the Heat Link:

1. The start-up handshake `bpd` already does (reset, wire ACK, queries, power steal).
2. Read `MSG_WIRES`: on the Heat Link backplate byte 0 is 1 (captured), on the bare backplate it
   was 0. Inferred: byte 0 is the circuit the Heat Link sits on.
3. Send `0x82 [circuit][1]`, wait for the ACK, listen for the relay; `0x82 [circuit][0]` to stop.
   From the firmware: circuit 0, state 1 = on, 0 = off, accepted once the wire ACK was sent.
   Still to be seen on the device: that the relay follows.
4. Keep the tick alive and clear the switch on exit, as stock does.

The certain way to fill the unknowns is the capture already planned: stock nlclient behind
ttyproxy on that backplate while the heat demand is changed (`tools/nlclient-mitm/runnlc.sh`).
