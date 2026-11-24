//! An authorized retrospective read across the spring billing-calendar change.
use smgw::{
    taf6::{History, Quantity, Register},
    taf7::{Channel, Direction},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let quantity = Quantity {
        profile_id: "import-billing",
        channel: Channel {
            meter_id: "demo-meter",
            direction: Direction::Import,
        },
        register: Register::Meter,
    };
    // Berlin local midnights, March 28–31, 2026. March 29 has 23 hours.
    let calendar = [1_774_652_400, 1_774_738_800, 1_774_821_600, 1_774_908_000];
    let mut history = History::new(quantity, &calendar)?;
    history.record(quantity, calendar[1], 545_000)?;
    history.record(quantity, calendar[2], 545_300)?;
    let report = history
        .lookup(quantity, calendar[1], calendar[2])?
        .ok_or("opening balance unavailable")?;
    println!(
        "{} | {} | opening {} UTC (Unix seconds) | {} Wh",
        report.quantity.profile_id, report.quantity.channel.meter_id, report.opening_utc, report.wh
    );
    Ok(())
}
