use smgw::taf15::{Channel, Period, Profile, Record};

fn main() -> Result<(), smgw::taf15::Error> {
    let channel = Channel {
        meter_id: "shop",
        obis: [1, 0, 1, 29, 0, 255],
        unit: "Wh",
    };
    let mut profile = Profile::<2>::new("meter-history", channel, Period::QuarterHour, 900)?;
    for (reference_utc, wh) in [(1800, 200), (900, 300), (900, 300)] {
        let record = Record {
            reference_utc,
            measured_utc: reference_utc - 1,
            coefficient: wh,
            scale: 0,
            meter_status: Some(0),
        };
        println!(
            "Capture at {reference_utc} s: {:?}",
            profile.capture(channel, record)?
        );
    }
    for record in profile.records() {
        println!(
            "slot {} s   meter time {} s   {} {}",
            record.reference_utc, record.measured_utc, record.coefficient, channel.unit
        );
    }
    Ok(())
}
