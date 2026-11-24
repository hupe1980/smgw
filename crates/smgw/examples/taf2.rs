//! Compare a measured tariff switch with the same switch during a reception gap.
use smgw::taf2::{Error, Reading, Registers, TariffRegister};

fn main() -> Result<(), Error> {
    let low = TariffRegister::new(1)?;
    let high = TariffRegister::new(2)?;
    for (label, boundary) in [
        ("Measured switch", Reading::Valid(1120)),
        ("Missing boundary", Reading::Missing),
    ] {
        let mut bank = Registers::new(9 * 3600 + 45 * 60, 1000)?;
        bank.record(10 * 3600, low, boundary)?;
        bank.record(10 * 3600 + 15 * 60, high, Reading::Valid(1200))?;
        println!(
            "{label}: total {} Wh, low {} Wh, high {} Wh, error {} Wh",
            bank.total_wh(),
            bank.tariff_wh(low),
            bank.tariff_wh(high),
            bank.error_wh()
        );
    }
    Ok(())
}
