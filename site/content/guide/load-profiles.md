+++
title = "Load profiles and maximum demand"
description = "Calculate interval demand from cumulative electricity readings with TAF 8, or backfill a meter’s own registered history with TAF 15."
weight = 2
+++

Choose the profile by where registration takes place. TAF 8 derives demand at the gateway. TAF 15 captures records that the meter already registered, including their original timestamps, units and status. Both examples run locally. [BSI TR-03109-1 v2.0, §§3.2.12–13](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR-03109-1_Anforderungen_Interoperabilitaet_2_0.pdf?__blob=publicationFile)

## Calculate demand with TAF 8

Run `cargo run -p smgw --example taf8`. The register rises from 1,000 Wh to 1,300 Wh in 15 minutes, giving **1,200 W**. The next interval adds 200 Wh and averages 800 W. The peak stays at 1,200 W with the time of the first interval’s end.

Commission one import, export or reactive-energy register. Supply the local daily or monthly billing boundaries as UTC seconds. Electricity profiles accept 5- or 15-minute slots. Record the opening balance, then every boundary through the close. The result retains the register’s decimal scale. A `scale` of −3 uses milliwatt-hours for active energy and milliwatts for demand.

Use `Reading::MISSING` for an absent endpoint. Both adjacent intervals carry a flagged zero and preserve the existing peak. A permanent fault freezes the peak through the close. The read-only `provisional` method gives the local average partway through a slot. Archive each completed interval and start the next billing window with its opening reading. [TAF 8 API](../../api/smgw/taf8/struct.Profile.html)

## Backfill meter records with TAF 15

Run `cargo run -p smgw --example taf15`. The meter supplies the second quarter-hour before the first. Importing the first record twice returns `Duplicate` on the retry. Reading the history yields **300 Wh, then 200 Wh**, each with its original meter time.

Commission one electricity or gas OBIS channel, its unit, the meter’s first reference instant, and the number of slots. Gas also supports hourly registration. Use `missing()` to select slots for a retry and `records()` to read the captured history in reference-time order. A conflicting record returns an error and the first record stays intact. The host authenticates the meter, stores the completed history, and authorizes delivery. [TAF 15 API](../../api/smgw/taf15/struct.Profile.html)
