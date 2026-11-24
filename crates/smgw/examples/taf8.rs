use smgw::taf8::{Channel, Direction, Period, Profile, Reading, Register};

fn main() -> Result<(), smgw::taf8::Error> {
    let channel = Channel {
        meter_id: "shop",
        register: Register::Active(Direction::Import),
        scale: 0,
    };
    let mut profile = Profile::new("daily-import", channel, Period::QuarterHour, 0, 86400)?;
    for (utc, wh) in [(0, 1000), (900, 1300), (1800, 1500)] {
        if let Some(interval) = profile.record(channel, utc, Reading::valid(wh))? {
            let peak = interval.peak.expect("positive measured demand");
            println!(
                "t={utc} s   {wh} Wh   mean {} W   peak {} W at {} s",
                interval.mean.count, peak.demand.count, peak.end_utc
            );
        }
    }
    Ok(())
}
