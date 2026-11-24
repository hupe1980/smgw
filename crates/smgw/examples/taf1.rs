//! Select a monthly close from the final two quarter-hours of a local TAF 7 block.
use smgw::{
    taf1::Profile,
    taf7::{Channel, Direction, Observation, Period, Quality, Series},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let channel = Channel {
        meter_id: "demo-meter",
        direction: Direction::Import,
    };
    let close = 1_772_323_200; // 2026-03-01 00:00 UTC
    let mut series = Series::<2>::new(channel, Period::QuarterHour, close - 900)?;
    for (scheduled_utc, cumulative_wh) in [(close - 900, 544_900), (close, 545_000)] {
        series.record(
            channel,
            scheduled_utc,
            Observation::Received {
                source_utc: scheduled_utc,
                cumulative_wh,
                quality: Quality::Valid,
            },
        )?;
    }
    let closes = [close];
    let mut profile = Profile::new("monthly-import", channel, &closes)?;
    let entry = series.entries().last().ok_or("expected a closing slot")?;
    let report = profile
        .close(series.channel(), *entry)?
        .ok_or("closing reading unavailable")?;
    println!(
        "{} | {} | close {} UTC (Unix seconds) | {} Wh",
        report.profile_id, report.channel.meter_id, report.end_utc, report.cumulative_wh
    );
    Ok(())
}
