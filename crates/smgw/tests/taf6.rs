use smgw::{
    taf2::TariffRegister,
    taf6::{Error, History, Quantity, Register},
    taf7::{Channel, Direction},
};

const METER: Quantity<'static> = Quantity {
    profile_id: "energy",
    channel: Channel {
        meter_id: "meter",
        direction: Direction::Import,
    },
    register: Register::Meter,
};

#[test]
fn retention_counts_local_days_and_never_reuses_a_stale_slot() {
    for change in [-3600, 3600] {
        let calendar: [i64; 90] =
            std::array::from_fn(|i| i as i64 * 86400 + if i >= 2 { change } else { 0 });
        let mut h = History::new(METER, &calendar).unwrap();
        h.record(METER, calendar[0], 0).unwrap();
        h.record(METER, calendar[1], 10).unwrap();
        assert_eq!(
            h.lookup(METER, calendar[0], calendar[42] - 1)
                .unwrap()
                .unwrap()
                .wh,
            0
        );
        assert_eq!(h.lookup(METER, calendar[0], calendar[42]).unwrap(), None);
        assert_eq!(
            h.lookup(METER, calendar[1], calendar[42])
                .unwrap()
                .unwrap()
                .wh,
            10
        );
        assert_eq!(h.lookup(METER, calendar[42], calendar[42]).unwrap(), None);
        assert_eq!(h.advance_to(calendar[41]), Err(Error::DayWentBackwards));
        h.record(METER, calendar[42], 420).unwrap();
        h.advance_to(calendar[86]).unwrap();
        assert_eq!(h.lookup(METER, calendar[84], calendar[86]).unwrap(), None);
        assert_eq!(h.lookup(METER, calendar[42], calendar[86]).unwrap(), None);
    }
}

#[test]
fn scope_and_exact_date_bind_the_response() {
    let register = Quantity {
        register: Register::DerivedTariff(TariffRegister::new(3).unwrap()),
        ..METER
    };
    let calendar = [-86400, 0, 86400, 172800];
    let mut h = History::new(register, &calendar).unwrap();
    assert_eq!(h.record(register, 1, 50), Err(Error::NotDayOpening));
    assert_eq!(h.record(METER, 0, 50), Err(Error::WrongQuantity));
    h.record(register, 0, 123).unwrap();
    assert_eq!(h.record(register, 0, 124), Err(Error::DayAlreadyRecorded));
    for wrong in [
        METER,
        Quantity {
            profile_id: "other",
            ..register
        },
        Quantity {
            channel: Channel {
                direction: Direction::Export,
                ..METER.channel
            },
            ..register
        },
    ] {
        assert_eq!(h.lookup(wrong, 0, 86400), Err(Error::WrongQuantity));
    }
    for day in [-86400, 1, 86400] {
        assert_eq!(h.lookup(register, day, 30).unwrap(), None);
    }
    let result = h.lookup(register, 0, 30).unwrap().unwrap();
    assert_eq!(
        (result.quantity, result.opening_utc, result.wh),
        (register, 0, 123)
    );
    assert_eq!(h.advance_to(172800), Err(Error::OutsideCalendar));
    for invalid in [[0, 0], [0, 2 * 86400], [i64::MIN, i64::MAX]] {
        assert!(matches!(
            History::new(register, &invalid),
            Err(Error::InvalidCalendar)
        ));
    }
}
