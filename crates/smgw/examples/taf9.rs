//! Report measured total active power on a one-minute grid.
use smgw::taf9::{ActivePower, Channel, Crossing, Profile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let channel = Channel {
        meter_id: "generator-meter",
        quantity: ActivePower::Total,
    };
    let mut profile = Profile::new("feed-in", channel, 0, true, Some(2_000))?;
    // This fixture uses positive watts for export.
    for (utc, watts) in [(0, 1_800), (60, 2_100), (120, 1_900)] {
        if let Some(event) = profile.record(channel, utc, Some(watts))? {
            let trigger = match event.crossing {
                Some(Crossing::Above) => "periodic, crossed above 2000 W",
                Some(Crossing::Below) => "periodic, crossed below 2000 W",
                None => "periodic",
            };
            println!(
                "{} | {} s | {} W | {trigger}",
                channel.quantity.obis(),
                event.reading.sampled_utc,
                event.reading.watts
            );
        }
    }
    let current = profile
        .on_demand(150)
        .ok_or("fresh feed-in reading unavailable")?;
    println!(
        "On demand at 150 s: {} W sampled at {} s",
        current.watts, current.sampled_utc
    );
    Ok(())
}
