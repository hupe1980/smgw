use smgw::{taf8, taf15};

const IMPORT: taf8::Channel<'static> = taf8::Channel {
    meter_id: "east",
    register: taf8::Register::Active(taf8::Direction::Import),
    scale: 0,
};
const METER: taf15::Channel<'static> = taf15::Channel {
    meter_id: "east",
    obis: [1, 0, 1, 29, 0, 255],
    unit: "Wh",
};

fn received(reference_utc: i64) -> taf15::Record {
    taf15::Record {
        reference_utc,
        measured_utc: reference_utc - 1,
        coefficient: 12507,
        scale: -2,
        meter_status: Some(0xf19a0002),
    }
}

#[test]
fn mean_peak_and_live_preview_use_the_correct_boundaries() {
    use taf8::{Period, Profile, Reading};
    let mut p = Profile::new("billing", IMPORT, Period::QuarterHour, 0, 2700).unwrap();
    assert_eq!(p.record(IMPORT, 0, Reading::valid(100)), Ok(None));
    // Half-up rounding of 1 Wh in 32 seconds gives 113 W.
    assert_eq!(
        p.provisional(IMPORT, 32, Reading::valid(101))
            .unwrap()
            .count,
        113
    );
    for t in [0, 900] {
        assert_eq!(
            p.provisional(IMPORT, t, Reading::valid(101)),
            Err(taf8::Error::UnexpectedSlot)
        );
    }
    let first = p.record(IMPORT, 900, Reading::valid(400)).unwrap().unwrap();
    assert_eq!(
        (
            first.profile_id,
            first.channel,
            first.closing,
            first.mean.count,
            first.peak.unwrap().end_utc
        ),
        ("billing", IMPORT, Reading::valid(400), 1200, 900)
    );
    let tie = p
        .record(IMPORT, 1800, Reading::valid(700))
        .unwrap()
        .unwrap();
    assert_eq!(tie.peak, first.peak);
    let last = p
        .record(IMPORT, 2700, Reading::valid(1001))
        .unwrap()
        .unwrap();
    assert_eq!((last.peak.unwrap().end_utc, last.mean.count), (2700, 1204));
    assert_eq!(
        p.record(IMPORT, 3600, Reading::MISSING),
        Err(taf8::Error::UnexpectedSlot)
    );
    let mut next = Profile::new("billing", IMPORT, Period::QuarterHour, 2700, 3600).unwrap();
    next.record(IMPORT, 2700, last.closing).unwrap();
    assert_eq!(
        next.record(IMPORT, 3600, last.closing)
            .unwrap()
            .unwrap()
            .peak,
        None
    );
}

// SR-001. Rejections leave the slot pending and faulty inputs cannot advance the peak.
#[test]
fn rejection_and_faults_preserve_the_billing_maximum() {
    use taf8::{Error, Period, Profile, Reading, Status};
    let mut p = Profile::new("p", IMPORT, Period::FiveMinutes, 0, 2700).unwrap();
    p.record(IMPORT, 0, Reading::valid(10)).unwrap();
    let warning = Reading {
        status: Status::Warning,
        validated_for_billing: false,
        count: 20,
    };
    let first = p.record(IMPORT, 300, warning).unwrap().unwrap();
    assert_eq!(
        first.mean,
        taf8::Demand {
            count: 120,
            status: Status::Warning,
            validated_for_billing: false
        }
    );
    for other in [
        taf8::Channel {
            meter_id: "west",
            ..IMPORT
        },
        taf8::Channel {
            scale: -2,
            ..IMPORT
        },
        taf8::Channel {
            register: taf8::Register::Active(taf8::Direction::Export),
            ..IMPORT
        },
    ] {
        assert_eq!(
            p.record(other, 600, Reading::valid(30)),
            Err(Error::WrongChannel)
        );
    }
    assert_eq!(
        p.record(IMPORT, 900, Reading::valid(30)),
        Err(Error::UnexpectedSlot)
    );
    assert_eq!(
        p.record(IMPORT, 600, Reading::valid(0)),
        Err(Error::CounterDecreased)
    );
    for (t, reading, status, count) in [
        (600, Reading::MISSING, Status::Missing, 0),
        (900, Reading::valid(100), Status::Missing, 0),
        (1200, Reading::valid(102), Status::Valid, 24),
        (
            1500,
            Reading {
                status: Status::TemporaryError,
                ..Reading::valid(9999)
            },
            Status::TemporaryError,
            0,
        ),
        (1800, Reading::valid(105), Status::TemporaryError, 0),
        (
            2100,
            Reading {
                status: Status::PermanentError,
                ..Reading::valid(105)
            },
            Status::PermanentError,
            0,
        ),
        (2400, Reading::valid(108), Status::PermanentError, 0),
        (2700, Reading::valid(u64::MAX), Status::PermanentError, 0),
    ] {
        let interval = p.record(IMPORT, t, reading).unwrap().unwrap();
        assert_eq!(
            (interval.mean.count, interval.mean.status, interval.peak),
            (count, status, first.peak)
        );
        assert_eq!(interval.mean.validated_for_billing, status == Status::Valid);
        if t >= 2100 {
            assert_eq!(interval.closing.status, Status::PermanentError);
            assert!(!interval.closing.validated_for_billing);
        }
    }
}

#[test]
fn all_six_register_families_keep_their_scale_and_full_precision() {
    use taf8::{Direction, Period, Profile, Quadrant, Reading, Register};
    for (register, c) in [
        (Register::Active(Direction::Import), 1),
        (Register::Active(Direction::Export), 2),
        (Register::Reactive(Quadrant::R1), 5),
        (Register::Reactive(Quadrant::R2), 6),
        (Register::Reactive(Quadrant::R3), 7),
        (Register::Reactive(Quadrant::R4), 8),
    ] {
        assert_eq!(register.energy_obis(), [1, 0, c, 8, 0, 255]);
        assert_eq!(register.demand_obis(), [1, 0, c, 29, 0, 255]);
        assert_eq!(register.maximum_obis(), [1, 0, c, 6, 0, 255]);
        let channel = taf8::Channel {
            register,
            scale: -3,
            ..IMPORT
        };
        let mut p = Profile::new("p", channel, Period::FiveMinutes, -300, 0).unwrap();
        p.record(channel, -300, Reading::valid(0)).unwrap();
        let interval = p
            .record(channel, 0, Reading::valid(u64::MAX))
            .unwrap()
            .unwrap();
        assert_eq!(interval.channel.scale, -3);
        assert_eq!(interval.mean.count, u128::from(u64::MAX) * 12);
    }
    for (period, open, close) in [
        (Period::Hour, 0, 3600),
        (Period::QuarterHour, 0, 0),
        (Period::FiveMinutes, 1, 300),
        (Period::FiveMinutes, -300, 1),
    ] {
        assert!(matches!(
            Profile::new("p", IMPORT, period, open, close),
            Err(taf8::Error::InvalidWindow)
        ));
    }
}

// SR-002. Backfill preserves original meter facts across replays and conflicts.
#[test]
fn overlapping_capture_preserves_scope_and_original_records() {
    use taf15::{Capture, Error, Period, Profile};
    let mut p = Profile::<3>::new("history", METER, Period::QuarterHour, 0).unwrap();
    let late = received(1800);
    assert_eq!(p.capture(METER, late), Ok(Capture::Inserted));
    for channel in [
        taf15::Channel {
            meter_id: "other",
            ..METER
        },
        taf15::Channel {
            unit: "kWh",
            ..METER
        },
        taf15::Channel {
            obis: [1, 0, 1, 8, 0, 255],
            ..METER
        },
    ] {
        assert_eq!(p.capture(channel, received(0)), Err(Error::WrongChannel));
    }
    assert_eq!(p.capture(METER, late), Ok(Capture::Duplicate));
    for changed in [
        taf15::Record { scale: 0, ..late },
        taf15::Record {
            coefficient: -1,
            ..late
        },
        taf15::Record {
            meter_status: None,
            ..late
        },
        taf15::Record {
            measured_utc: late.reference_utc,
            ..late
        },
        taf15::Record {
            reference_utc: 900,
            ..late
        },
    ] {
        assert_eq!(p.capture(METER, changed), Err(Error::Conflict));
    }
    assert_eq!(p.missing().collect::<Vec<_>>(), [0, 900]);
    let first = taf15::Record {
        meter_status: None,
        coefficient: -127,
        ..received(0)
    };
    p.capture(METER, first).unwrap();
    p.capture(METER, received(900)).unwrap();
    assert_eq!(
        p.records().copied().collect::<Vec<_>>(),
        [first, received(900), late]
    );
    assert_eq!(
        (p.profile_id(), p.channel(), p.missing().count()),
        ("history", METER, 0)
    );
}

// SR-003. The window remains bounded even for extreme or off-grid timestamps.
#[test]
fn capture_bounds_and_medium_specific_periods() {
    use taf15::{Error, Period, Profile};
    let gas = taf15::Channel {
        obis: [7, 0, 3, 1, 0, 255],
        unit: "m³",
        ..METER
    };
    let mut p = Profile::<3>::new("gas", gas, Period::Hour, i64::MIN).unwrap();
    for t in [i64::MIN + 1, i64::MIN + 10800, i64::MAX] {
        assert_eq!(p.capture(gas, received(t)), Err(Error::UnexpectedSlot));
    }
    assert_eq!(
        p.missing().collect::<Vec<_>>(),
        [i64::MIN, i64::MIN + 3600, i64::MIN + 7200]
    );
    assert_eq!(p.records().count(), 0);
    assert!(matches!(
        Profile::<2>::new("g", gas, Period::Hour, i64::MAX),
        Err(Error::InvalidWindow)
    ));
    assert!(Profile::<1>::new("g", gas, Period::Hour, i64::MAX).is_ok());
    assert!(matches!(
        Profile::<0>::new("g", gas, Period::Hour, 0),
        Err(Error::InvalidWindow)
    ));
    assert!(matches!(
        Profile::<1>::new("p", METER, Period::Hour, 0),
        Err(Error::InvalidWindow)
    ));
    for obis in [
        [2, 0, 1, 8, 0, 255],
        [1, 2, 1, 8, 0, 255],
        [1, 0, 1, 8, 0, 3],
    ] {
        assert!(matches!(
            Profile::<1>::new(
                "p",
                taf15::Channel { obis, ..METER },
                Period::FiveMinutes,
                0
            ),
            Err(Error::InvalidChannel)
        ));
    }
}
