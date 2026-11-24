+++
title = "Electricity metering"
description = "Run Rust examples for the seven BSI v1.1 metering use cases, from monthly billing closes to TAF 14 measurement blocks for value-added services."
weight = 1
+++

A tariff use case (German *Tarifanwendungsfall*, TAF) specifies what the gateway does with meter data. The billing examples use cumulative electricity readings in whole watt-hours (Wh). TAF 9 and TAF 10 use instantaneous measurements. For gateway-calculated demand and meter-registered histories, see the [TAF 8 and TAF 15 guide](../load-profiles/).

## Select a TAF 1 closing reading

Run `cargo run -p smgw --example taf1`. The meter reports 544,900 Wh shortly before the monthly close and 545,000 Wh at the close. The output contains one value, **545,000 Wh**, with the profile ID, meter channel and scheduled closing time.

Commission one import or export channel and resolve the local monthly or multi-month calendar into ordered UTC closes. Resolve calendar boundaries in the operator’s time zone, including daylight-saving transitions. Call `Profile::close` after the reception window ends. A wrong channel or slot leaves the close pending. A finalized missing or invalid reading consumes that close and yields no report. Keep durable delivery state in the caller if the application must resume after a restart.

The simulator selects the closing value. The application owns meter authentication, original-value retention, recipient authorization, signing and delivery. [Closing profile API](../../api/smgw/taf1/struct.Profile.html) · [BSI TR-03109-1 v1.1, §4.2.2](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR03109-1.pdf?__blob=publicationFile&v=4)

## Simulate TAF 2 energy registers

Run `cargo run -p smgw --example taf2` to replay a single cumulative electricity meter. At 09:45 UTC the baseline is 1,000 Wh. The tariff changes at 10:00. At 10:15 the meter reads 1,200 Wh.

| Reading at 10:00 | Outgoing register | Incoming register | Error register (63) | Total (0) |
| --- | ---: | ---: | ---: | ---: |
| 1,120 Wh | 120 Wh | 80 Wh | 0 Wh | 200 Wh |
| Missing | 0 Wh | 0 Wh | 200 Wh | 200 Wh |

Create one accumulator per meter, energy direction and evaluation profile. Submit every quarter-hour in order, including missing slots. Supply the register that applied throughout the **elapsed** period. A gap wholly within one tariff credits that register on recovery. A change anywhere inside a gap makes the recovered delta ambiguous. A nonfatal meter fault sends both adjacent measured deltas to the error register. The simulator stops the instance on a fatal fault.

The caller selects the authenticated meter, resolves the schedule and classifies status and timeliness. FSCH can supply price windows. Commissioning determines the corresponding metering register. The model covers derived-register arithmetic; original-value recording, OBIS provisioning, signed reports and billing belong to the surrounding system. [Register API](../../api/smgw/taf2/index.html) · [BSI TR-03109-1 v1.1, §§4.2.3.2–5](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR03109-1.pdf?__blob=publicationFile&v=4)

## Retrieve a TAF 6 daily balance

Run `cargo run -p smgw --example taf6`. The example retrieves **545,000 Wh** for the opening of March 29, 2026 in Berlin. The next day opens 23 hours later. Each history holds one profile, meter, import or export direction, and register. Use the physical meter balance or a derived total, tariff or error register.

Supply consecutive local day boundaries resolved to UTC. Record qualified balances at the exact opening and call `advance_to` as billing days pass, including days with missing readings. A lookup uses the trusted current time and returns a single exact match within the current day and its 41 predecessors. Authorize the exceptional request for that recipient, quantity and date before lookup. The application handles durable retention, the HAN access log, signing, encryption and delivery. [History API](../../api/smgw/taf6/struct.History.html) · [BSI v1.1, §4.2.4](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR03109-1.pdf?__blob=publicationFile&v=4)

## Record a TAF 7 meter series

Run `cargo run -p smgw --example taf7` to record 1,000 Wh at 09:45 UTC, a missing slot at 10:00 and 1,200 Wh at 10:15. All three entries remain visible after recovery. With a tariff switch at 10:00, TAF 2 books the 200 Wh difference to its error register.

Create a `Series::<N>` for one commissioned meter and import or export direction. Choose quarter-hour or hourly slots, starting on the UTC grid. Submit every slot in sequence, with its scheduled time. For a received reading supply its source timestamp, cumulative whole Wh and quality. Use `Observation::Missing` for a gap. Archive the block when full and begin the next one explicitly.

This simulator covers local recording for a single active-energy channel. The caller resolves the meter ID and OBIS quantity, validates origin and status, and handles retention, recipient authorization and signed delivery. Only quarter-hour traces match this workspace’s TAF 2 period. [Series API](../../api/smgw/taf7/struct.Series.html) · [BSI TR-03109-1 v1.1, §4.2.5](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR03109-1.pdf?__blob=publicationFile&v=4)

## Report TAF 9 feed-in power

Run `cargo run -p smgw --example taf9`. The generation meter reports 1,800, 2,100 and 1,900 W at one-minute intervals. With a 2,000 W threshold, the second reading crosses above and the third crosses below. The sample identifies the triggering channel even when a periodic report falls at the same instant.

Commission one meter and one active-power quantity (total, L1, L2 or L3). Use its signed watt convention for both measurements and threshold. Pass `None` for a missing or invalid sample. The first qualified sample after a gap establishes a new baseline. An on-demand lookup expires at the next reception slot. This profile emits individual samples. The host handles recipient authorization, transmission and logging. [Feed-in profile API](../../api/smgw/taf9/struct.Profile.html) · [BSI v1.1, §4.2.6 and table 4.7](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR03109-1.pdf?__blob=publicationFile&v=4)

## Report TAF 10 grid-state measurements

Run `cargo run -p smgw --example taf10`. L1 voltage rises from 239.5 V to 240.5 V. The second periodic report also signals the upward crossing of 240 V. Its meter reference is the commissioned pseudonym `feeder-17`. The same sample is available on demand until the next minute begins.

Commission one meter and one of the 16 permitted OBIS channels per profile. Normalize active power to signed watts, current to milliamps, voltage to millivolts, frequency to millihertz and phase differences to signed millidegrees. Use the same angle convention for the threshold and the samples. Submit a value or a gap for each 60-second slot. A wrong source or unit leaves the slot open for correction.

The profile emits individual readings. To use a pseudonym, supply it at commissioning and choose a profile ID that does not identify the meter. The host authenticates the meter, enforces the profile’s validity and recipient permissions, and delivers reports. After on-demand delivery, log the recipient and omit the measured value. [Grid-state profile API](../../api/smgw/taf10/struct.Profile.html) · [BSI TR-03109-1 v1.1, §4.2.7](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR03109-1.pdf?__blob=publicationFile&v=4)

## Deliver TAF 14 measurement blocks

Run `cargo run -p smgw --example taf14`. A customer portal receives **800, 950 and 1,050 W** in one block. Its third sample crosses 1,000 W just as the three-slot period ends. The block identifies that sample and both reasons for delivery.

Commission one active-energy register (Wh), reactive-energy register (varh) or instantaneous active-power channel (W). By default, each qualified reading is ready for delivery on arrival, with one reception slot per minute. Set `delivery_every_slots` for periodic blocks or supply increasing thresholds for event delivery. Submit gaps as `None`. A gap breaks the threshold comparison and still counts toward the periodic boundary.

Choose `Profile::<N>` to hold up to N qualified samples. A periodic block must fit in full. If a threshold-only profile waits longer than its buffer can hold, the block returns the latest N readings in order and counts displaced readings in `omitted`. Consume or copy the block before submitting the next slot. The host checks customer consent and recipient permissions, records the delivery configuration without measured values in the customer log, and handles transmission and retries. [Measurement block API](../../api/smgw/taf14/struct.Profile.html) · [BSI v1.1, §4.2.8 and table 4.11](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR03109-1.pdf?__blob=publicationFile&v=4)
