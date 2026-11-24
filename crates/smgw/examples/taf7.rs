//! Replay a meter across a tariff switch with a missing boundary observation.
use smgw::{
    taf2::{Registers, TariffRegister},
    taf7::{Channel, Direction, Observation, Period, Quality, Series},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let meter = Channel {
        meter_id: "demo-meter",
        direction: Direction::Import,
    };
    let first = 9 * 3600 + 45 * 60;
    let sample = |source_utc, cumulative_wh| Observation::Received {
        source_utc,
        cumulative_wh,
        quality: Quality::Valid,
    };
    let mut series = Series::<3>::new(meter, Period::QuarterHour, first)?;
    series.record(meter, first, sample(first - 1, 1000))?;
    series.record(meter, first + 900, Observation::Missing)?;
    series.record(meter, first + 1800, sample(first + 1800, 1200))?;
    println!("Meter {} (import), UTC", series.channel().meter_id);
    for entry in series.entries() {
        let hour = entry.scheduled_utc / 3600;
        let minute = entry.scheduled_utc / 60 % 60;
        match entry.observation {
            Observation::Missing => println!("{hour:02}:{minute:02}  missing"),
            Observation::Received {
                source_utc,
                cumulative_wh,
                quality,
            } => println!(
                "{hour:02}:{minute:02}  {cumulative_wh} Wh  {quality:?}  (source UTC second {source_utc})"
            ),
        }
    }

    let (baseline, periods) = series.entries().split_first().unwrap();
    let Observation::Received {
        cumulative_wh,
        quality: Quality::Valid,
        ..
    } = baseline.observation
    else {
        return Err("expected a valid baseline".into());
    };
    let mut registers = Registers::new(baseline.scheduled_utc, cumulative_wh)?;
    let tariffs = [TariffRegister::new(1)?, TariffRegister::new(2)?];
    for (entry, tariff) in periods.iter().zip(tariffs) {
        registers.record(entry.scheduled_utc, tariff, entry.observation.into())?;
    }
    println!(
        "TAF 2: total {} Wh, error register {} Wh",
        registers.total_wh(),
        registers.error_wh()
    );
    Ok(())
}
