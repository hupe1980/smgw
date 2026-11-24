use smgw::taf14::{ActivePower, Channel, Config, Profile, Quantity, Value};

fn main() -> Result<(), smgw::taf14::Error> {
    let channel = Channel {
        meter_id: "demo-meter",
        quantity: Quantity::ActivePower(ActivePower::Total),
    };
    let mut profile = Profile::<3>::new(Config {
        delivery_every_slots: Some(3),
        thresholds: &[Value::Watts(1_000)],
        ..Config::new("customer-portal", channel, 0)
    })?;

    for (utc, watts) in [(0, 800), (60, 950), (120, 1_050)] {
        if let Some(block) = profile.record(channel, utc, Some(Value::Watts(watts)))? {
            println!(
                "{} · {} · {} readings",
                block.profile_id,
                block.channel.quantity.obis(),
                block.samples.len()
            );
            for sample in block.samples {
                if let Value::Watts(w) = sample.value {
                    println!("t={} s   {w} W", sample.sampled_utc);
                }
            }
            if let Some(crossing) = block.crossing {
                println!(
                    "Threshold crossing {:?} at t={} s · periodic={}",
                    crossing.direction, crossing.sampled_utc, block.periodic
                );
            }
        }
    }
    Ok(())
}
