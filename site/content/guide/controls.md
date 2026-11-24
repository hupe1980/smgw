+++
title = "EEBUS and relay controls"
description = "Map IEC 61850 operations to EEBUS power limits or relay contacts, read independent feedback, and import FSCH tariff schedules."
weight = 4
+++

## Bridge power limits and measurements

Enable the bridge with `cargo add smgw-cls --features fnn-bridge`. Configure the IEC 61850 control reference, EEBUS destination and reference power. Limitation of Power Consumption (LPC) and Limitation of Power Production (LPP) use opposite signs. At 40% of 10 kW, an LPC consumption limit is +4,000 W. An LPP production limit is −4,000 W on the wire. Zero percent activates a zero-watt limit. Use the separately commissioned release control to deactivate it. [EEBUS LPP sign conventions, §2.8.1](https://www.eebus.org/wp-content/uploads/2023/04/EEBus_UC_TS_LimitationOfPowerProduction_V1.0.0_public.pdf)

Use `LpcBridge::schedule` with a ready consumption guard. For LPP, `LppBridge::write` enqueues a SPINE write with signed watts on the commissioned connection. Match its message counter with the peer’s result. Read the peer’s limit descriptions and current limit list during commissioning. The driver owns authorization, heartbeats and retry policy. [LPC API](../../api/smgw_cls/bridge/struct.LpcBridge.html) · [LPP API](../../api/smgw_cls/bridge/struct.LppBridge.html)

For readback, select **MGCP** for net active power at the grid connection point or **MPC** for an individual device. Import or consumption is positive. Export or production is negative. Commission each source against its authenticated peer, entity and watt descriptor. Recommission after a description change and call `disconnect` when the session closes. Read the signed watt value at `MMXU1$MX$TotW$mag$f`, its quality at `TotW$q`, and its source time at `TotW$t`. Configure the maximum sample age. Missing, invalid or expired data makes the watt value unavailable. This profile requires an absolute timestamp with an explicit time zone. [Readback API](../../api/smgw_cls/bridge/measurement/struct.PowerReadback.html) · [EEBUS use-case roles](https://www.eebus.org/certification-faq/)

Run the local exchange from the repository root. It sends a 40% production command, checks the peer's acknowledgement, and reads an independent −2,700 W grid sample over MMS.

```sh
just bridge
```

## Simulate relay controls and tariff windows

Run the example from the repository root. W4 stays dimmed across tariff changes. At UTC second 5400, the requested generation stage is 30% and the independent feedback sample reports 60%.

```sh
cargo run --locked -p smgw-cls --no-default-features --example relay_tariff
```

| Generation limit | Active generation contact |
| --- | --- |
| 100% | None |
| 60% | S1 |
| 30% | S2 |
| 0% | W3 |

W4 independently requests load dimming. Initialize both settings explicitly. A request for 40% fails and preserves the previous state. Select the relay or EEBUS interface during commissioning. Losing a connection does not transfer control. [VDE FNN relay assignments, §6](https://www.vde.com/resource/blob/2380710/0220c532a4ecec2b29c25c52858290dc/vde-fnn-hinweis-schnittstellen-steuerungseinrichtung-data.pdf)

With `fnn-bridge`, map the generation percentage and Boolean dimming command to two distinct MMS control references. For status reads, commission four Boolean status leaves and supply independently sampled contacts. Pass `None` for missing or stale feedback. Conflicting indications remain visible for diagnosis. [Relay adapter API](../../api/smgw_cls/bridge/relay/index.html)

For Module 3, supply the grid operator’s standard, low and high windows as consecutive UTC intervals. Convert the operator’s local calendar, including daylight-saving changes, before constructing the schedule. The end of each interval is exclusive. Outside the supplied period the tariff is unknown. Looking up a tariff leaves active limits in force. [Tariff API](../../api/smgw_cls/fnn/tariff/index.html) · [Bundesnetzagentur guidance](https://www.bundesnetzagentur.de/DE/Vportal/Energie/SteuerbareVBE/Netzentgelt_table.html)

### Import a daily price schedule over MMS

Run `just tariff` to read a synthetic delivery day through the loopback gateway. The EMT reads three prices from `CLSLD0/FSCH1` over MMS. Each window includes its start and excludes its end.

```text
UTC interval (Unix seconds)  Tariff level
[86400, 115200)  Standard
[115200, 144000)  Low
[144000, 172800)  High
```

For your own reader, enable `fsch-tariff`. Configure the FSCH logical node, price objects and price unit from the operator’s contract. Pass the UTC boundaries of its local delivery day, including 23- and 25-hour days at daylight-saving transitions.

Use `read_tariff_windows` with a blocking client. On an existing async association, pass a `TariffRead` and a batch-read callback to `read_tariff_windows_async`. The callback sends one MMS Read for the requested fields and returns their decoded values in order. The association driver handles invoke IDs, deadlines and cancelled requests. [Blocking reader](../../api/smgw_cls/fnn/fsch/fn.read_tariff_windows.html) · [Async reader](../../api/smgw_cls/fnn/fsch/fn.read_tariff_windows_async.html)

Both paths recheck the schedule header before passing its original UTC start to your clock policy. To require a working, synchronized clock, use `|t| !t.quality.clock_failure && !t.quality.clock_not_synchronized`. The provider must keep prices stable during the read. Map each price to its commissioned level and replace the current windows only when the entire import succeeds.
