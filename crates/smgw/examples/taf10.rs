use smgw::taf10::{Channel, Config, Crossing, Phase, Profile, Quantity, Value};

fn main() -> Result<(), smgw::taf10::Error> {
    let channel = Channel {
        meter_id: "meter-0483",
        quantity: Quantity::Voltage(Phase::L1),
    };
    let mut profile = Profile::new(Config {
        profile_id: "feeder-voltage",
        channel,
        pseudonym: Some("feeder-17"),
        first_utc: 0,
        periodic: true,
        threshold: Some(Value::Millivolts(240_000)),
    })?;
    for (utc, millivolts) in [(0, 239_500), (60, 240_500)] {
        let event = profile
            .record(channel, utc, Some(Value::Millivolts(millivolts)))?
            .expect("periodic reporting");
        let reading = event.reading;
        println!(
            "{} · {} · {} · t={} s",
            reading.meter_reference,
            reading.quantity.obis(),
            reading.value,
            reading.sampled_utc
        );
        if event.crossing == Some(Crossing::Above) {
            println!("Voltage crossed above 240000 mV");
        }
    }
    let latest = profile.on_demand(95).expect("sample is fresh");
    println!(
        "On demand at t=95 s: {} · {}",
        latest.meter_reference, latest.value
    );
    assert!(profile.on_demand(120).is_none());
    println!("At t=120 s the last reading has expired.");
    Ok(())
}
