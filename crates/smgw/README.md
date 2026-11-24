# smgw

Metering and management primitives for a smart meter gateway. The metering models work in `no_std` without allocating. Each example runs from the workspace root with `cargo run -p smgw --example <name>`.

| Example | What it shows |
| --- | --- |
| `taf1` | Select one cumulative reading at the commissioned monthly billing close. |
| `taf2` | Assign measured energy to tariff registers across a switch. |
| `taf6` | Retrieve one daily opening balance from six weeks of billing history. |
| `taf7` | Preserve the original readings, timestamps, quality and missing slots for one electricity channel. |
| `taf8` | Calculate mean and maximum demand from registered electricity readings. |
| `taf9` | Report instantaneous measured active power periodically, on demand or at a threshold crossing. |
| `taf10` | Report grid voltage under a pseudonym and detect a threshold crossing. Also supports active power, current, frequency and phase angles. |
| `taf14` | Deliver a block of readings and identify the sample that crossed a service threshold. |
| `taf15` | Backfill the meter’s registered history and replay overlapping reads. |

For gateway administrator wake-ups, enable `wakeup` to parse a datagram, `wakeup-verify` to authenticate it with OpenSSL, or `wakeup-store` to persist its replay watermark in redb. Run `just wakeup` to verify the bundled fixture.

The allocation-free CLS session engine is available under `smgw::cls`. For the TLS simulator and protocol adapters, enable the corresponding features on `smgw-cls`.

[Metering guide](https://hupe1980.github.io/smgw/guide/metering/) · [Load profiles](https://hupe1980.github.io/smgw/guide/load-profiles/) · [Wake-up verification](https://hupe1980.github.io/smgw/guide/wakeup/) · [Changelog](https://github.com/hupe1980/smgw/blob/main/CHANGELOG.md)
