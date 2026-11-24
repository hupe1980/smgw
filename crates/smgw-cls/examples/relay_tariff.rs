//! Replay a short synthetic operator schedule alongside independent relay commands.
use smgw_cls::fnn::{
    relay::{ConsumptionMode, GenerationStage, RelayContacts, RelayState},
    tariff::{TariffLevel, TariffSchedule, TariffWindow},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let windows = [
        TariffWindow {
            start_utc: 0,
            end_utc: 3600,
            level: TariffLevel::Standard,
        },
        TariffWindow {
            start_utc: 3600,
            end_utc: 7200,
            level: TariffLevel::Low,
        },
        TariffWindow {
            start_utc: 7200,
            end_utc: 10800,
            level: TariffLevel::High,
        },
    ];
    let schedule = TariffSchedule::new(&windows)?;
    let mut desired = RelayState::new(GenerationStage::Percent100, ConsumptionMode::Dimmed);
    println!("UTC second | tariff   | generation | desired S1 S2 W3 W4");
    for (utc, percent) in [
        (0, 100.),
        (3600, 60.),
        (5400, 30.),
        (7200, 0.),
        (10800, 100.),
    ] {
        desired.generation = GenerationStage::from_percent(percent)?;
        let level = match schedule.active_at(utc).map(|window| window.level) {
            Some(TariffLevel::Standard) => "standard",
            Some(TariffLevel::Low) => "low",
            Some(TariffLevel::High) => "high",
            None => "unknown",
        };
        let contacts = desired.contacts();
        let [s1, s2, w3, w4] = [contacts.s1, contacts.s2, contacts.w3, contacts.w4].map(u8::from);
        println!("{utc:>10} | {level:<8} | {percent:>9}% |         {s1}  {s2}  {w3}  {w4}");
    }
    // A separate synthetic feedback sample taken while the requested stage was 30%.
    let observed = RelayContacts {
        s1: true,
        s2: false,
        w3: false,
        w4: true,
    };
    let feedback = observed.decode()?;
    println!(
        "Observed at UTC second 5400: generation {}%, consumption {:?}.",
        feedback.generation.percent(),
        feedback.consumption
    );
    Ok(())
}
