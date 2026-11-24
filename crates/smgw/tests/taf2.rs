use smgw::taf2::{Error, Outcome, Reading, Registers, TariffRegister};

#[test]
fn invalid_input_is_atomic() {
    for number in [0, 63, 64, 255] {
        assert_eq!(TariffRegister::new(number), Err(Error::InvalidRegister));
    }
    assert!(matches!(
        Registers::new(1, 0),
        Err(Error::UnalignedBaseline)
    ));
    let a = TariffRegister::new(1).unwrap();
    let b = TariffRegister::new(62).unwrap();
    let mut regs = Registers::new(-900, 1).unwrap();
    regs.record(0, a, Reading::Missing).unwrap();
    for (end, reading, error) in [
        (0, Reading::Fatal, Error::UnexpectedSlot),
        (1800, Reading::Valid(4), Error::UnexpectedSlot),
        (900, Reading::MeterFault(0), Error::DecreasingReading),
    ] {
        assert_eq!(regs.record(end, b, reading), Err(error));
    }
    regs.record(900, a, Reading::Valid(u64::MAX)).unwrap();
    assert_eq!(regs.tariff_wh(a), u64::MAX - 1);
    assert_eq!(regs.total_wh(), regs.tariff_wh(a));
    assert_eq!(regs.error_wh(), 0);
    assert_eq!(regs.record(1800, b, Reading::Fatal), Ok(Outcome::Stopped));
    assert_eq!(
        regs.record(2700, b, Reading::Valid(u64::MAX)),
        Err(Error::MeterStopped)
    );
    let mut edge = Registers::new(i64::MAX - i64::MAX % 900, 0).unwrap();
    assert_eq!(
        edge.record(i64::MIN, a, Reading::Valid(1)),
        Err(Error::UnexpectedSlot)
    );
}

#[test]
fn gap_and_fault_attribution() {
    let a = TariffRegister::new(1).unwrap();
    let b = TariffRegister::new(2).unwrap();
    let mut regs = Registers::new(0, 1000).unwrap();
    let trace = [
        (a, Reading::Valid(1120), 120, 0, 0),
        (b, Reading::Valid(1200), 120, 80, 0),
        (b, Reading::Missing, 120, 80, 0),
        (b, Reading::Valid(1250), 120, 130, 0),
        (a, Reading::Missing, 120, 130, 0),
        (b, Reading::TimeInvalid, 120, 130, 0),
        (a, Reading::Valid(1280), 120, 130, 30),
        (a, Reading::MeterFault(1300), 120, 130, 50),
        (a, Reading::MeterFault(1310), 120, 130, 60),
        (a, Reading::Missing, 120, 130, 60),
        (a, Reading::Valid(1330), 120, 130, 80),
        (b, Reading::Valid(1335), 120, 135, 80),
    ];
    for (i, (reg, reading, aw, bw, ew)) in trace.into_iter().enumerate() {
        regs.record((i as i64 + 1) * 900, reg, reading).unwrap();
        assert_eq!(
            (regs.tariff_wh(a), regs.tariff_wh(b), regs.error_wh()),
            (aw, bw, ew)
        );
        assert_eq!(regs.total_wh(), aw + bw + ew);
    }
}
