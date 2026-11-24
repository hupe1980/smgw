use smgw::taf14::{ActivePower, Channel, Config, Crossing, Error, Profile, Quantity, Value};

const POWER: Channel<'static> = Channel {
    meter_id: "service-meter",
    quantity: Quantity::ActivePower(ActivePower::Total),
};

#[test]
fn periodic_and_threshold_emit_one_attributed_block() {
    let mut p = Profile::<3>::new(Config {
        delivery_every_slots: Some(3),
        thresholds: &[Value::Watts(-10), Value::Watts(10)],
        ..Config::new("portal", POWER, 0)
    })
    .unwrap();
    for (utc, watts) in [(0, -10), (60, -10)] {
        assert!(
            p.record(POWER, utc, Some(Value::Watts(watts)))
                .unwrap()
                .is_none()
        );
    }
    let block = p
        .record(POWER, 120, Some(Value::Watts(20)))
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            block.profile_id,
            block.channel,
            block.periodic,
            block.omitted
        ),
        ("portal", POWER, true, 0)
    );
    assert_eq!(block.samples.len(), 3);
    let crossing = block.crossing.unwrap();
    assert_eq!(
        (crossing.sampled_utc, crossing.direction),
        (120, Crossing::Above)
    );
    assert_eq!(crossing.thresholds, [Value::Watts(-10), Value::Watts(10)]);
}

#[test]
fn every_table_4_11_channel_delivers_immediately_in_its_unit() {
    use Quantity::{ActiveEnergy, ReactiveEnergy};
    use smgw::taf14::{
        Direction::{Export, Import},
        Phase::{L1, L2, L3},
        Quadrant::{R1, R2, R3, R4},
    };
    let wh = Value::WattHours(u64::MAX);
    let varh = Value::VarHours(u64::MAX);
    let w = Value::Watts(i64::MIN);
    for (quantity, obis, value) in [
        (ActiveEnergy(Import, None), "1-0:1.8.0.255", wh),
        (ActiveEnergy(Import, Some(L1)), "1-0:21.8.0.255", wh),
        (ActiveEnergy(Import, Some(L2)), "1-0:41.8.0.255", wh),
        (ActiveEnergy(Import, Some(L3)), "1-0:61.8.0.255", wh),
        (ActiveEnergy(Export, None), "1-0:2.8.0.255", wh),
        (ActiveEnergy(Export, Some(L1)), "1-0:22.8.0.255", wh),
        (ActiveEnergy(Export, Some(L2)), "1-0:42.8.0.255", wh),
        (ActiveEnergy(Export, Some(L3)), "1-0:62.8.0.255", wh),
        (ReactiveEnergy(R1), "1-0:5.8.0.255", varh),
        (ReactiveEnergy(R2), "1-0:6.8.0.255", varh),
        (ReactiveEnergy(R3), "1-0:7.8.0.255", varh),
        (ReactiveEnergy(R4), "1-0:8.8.0.255", varh),
        (
            Quantity::ActivePower(ActivePower::Total),
            "1-0:16.7.0.255",
            w,
        ),
        (Quantity::ActivePower(ActivePower::L1), "1-0:36.7.0.255", w),
        (Quantity::ActivePower(ActivePower::L2), "1-0:56.7.0.255", w),
        (Quantity::ActivePower(ActivePower::L3), "1-0:76.7.0.255", w),
    ] {
        let channel = Channel { quantity, ..POWER };
        let mut p = Profile::<1>::new(Config::new("portal", channel, 0)).unwrap();
        assert_eq!(quantity.obis(), obis);
        for wrong in [w, wh, varh].into_iter().filter(|&v| v != value) {
            assert_eq!(p.record(channel, 0, Some(wrong)), Err(Error::WrongUnit));
        }
        let block = p.record(channel, 0, Some(value)).unwrap().unwrap();
        assert_eq!(
            block.samples,
            [smgw::taf14::Sample {
                sampled_utc: 0,
                value
            }]
        );
        assert!(!block.periodic);
        assert_eq!(block.crossing, None);
        assert_eq!(p.record(channel, 60, None), Ok(None));
    }
}

// TAF 14 SR-001. Rejected input can be corrected at the same slot.
#[test]
fn rejected_inputs_do_not_change_the_pending_block() {
    let mut p = Profile::<2>::new(Config {
        delivery_every_slots: Some(2),
        ..Config::new("portal", POWER, 0)
    })
    .unwrap();
    assert_eq!(p.record(POWER, 0, Some(Value::Watts(42))), Ok(None));
    for other in [
        Channel {
            meter_id: "other",
            ..POWER
        },
        Channel {
            quantity: Quantity::ActivePower(ActivePower::L1),
            ..POWER
        },
    ] {
        assert_eq!(p.record(other, 60, None), Err(Error::WrongChannel));
    }
    assert_eq!(
        p.record(POWER, 60, Some(Value::WattHours(0))),
        Err(Error::WrongUnit)
    );
    for wrong_utc in [0, 120] {
        assert_eq!(p.record(POWER, wrong_utc, None), Err(Error::UnexpectedSlot));
    }
    let block = p.record(POWER, 60, Some(Value::Watts(5))).unwrap().unwrap();
    assert_eq!(
        block
            .samples
            .iter()
            .map(|s| (s.sampled_utc, s.value))
            .collect::<Vec<_>>(),
        [(0, Value::Watts(42)), (60, Value::Watts(5))]
    );
}

// TAF 14 SR-002. A missing boundary flushes only this period's qualified samples.
#[test]
fn gaps_preserve_periodic_boundaries_and_scope() {
    let mut p = Profile::<3>::new(Config {
        reception_seconds: 1,
        delivery_every_slots: Some(3),
        thresholds: &[Value::Watts(0)],
        ..Config::new("portal", POWER, 10)
    })
    .unwrap();
    assert_eq!(p.record(POWER, 10, Some(Value::Watts(0))), Ok(None));
    let early = p.record(POWER, 11, Some(Value::Watts(1))).unwrap().unwrap();
    assert_eq!((early.periodic, early.samples.len()), (false, 2));
    assert_eq!(p.record(POWER, 12, None), Ok(None));
    assert_eq!(p.record(POWER, 13, Some(Value::Watts(-1))), Ok(None));
    assert_eq!(p.record(POWER, 14, None), Ok(None));
    let next = p.record(POWER, 15, None).unwrap().unwrap();
    assert_eq!(
        (next.profile_id, next.channel, next.periodic, next.crossing),
        ("portal", POWER, true, None)
    );
    assert_eq!(
        next.samples,
        [smgw::taf14::Sample {
            sampled_utc: 13,
            value: Value::Watts(-1)
        }]
    );
    for utc in 16..=18 {
        assert_eq!(p.record(POWER, utc, None), Ok(None));
    }
}

// TAF 14 SR-003. Wrapping retains the chronological suffix and counts every omitted sample.
#[test]
fn long_threshold_waits_report_omissions_and_reset_after_delivery() {
    let mut p = Profile::<3>::new(Config {
        reception_seconds: 1,
        thresholds: &[Value::Watts(0), Value::Watts(10)],
        ..Config::new("portal", POWER, 0)
    })
    .unwrap();
    for utc in 0..7 {
        assert_eq!(p.record(POWER, utc, Some(Value::Watts(-1))), Ok(None));
    }
    let up = p.record(POWER, 7, Some(Value::Watts(10))).unwrap().unwrap();
    assert_eq!(up.omitted, 5);
    assert_eq!(
        up.samples.iter().map(|s| s.sampled_utc).collect::<Vec<_>>(),
        [5, 6, 7]
    );
    assert_eq!(up.crossing.unwrap().thresholds, [Value::Watts(0)]);
    let down = p.record(POWER, 8, Some(Value::Watts(-1))).unwrap().unwrap();
    assert_eq!(down.samples.len(), 1);
    assert_eq!(down.omitted, 0);
    let crossing = down.crossing.unwrap();
    assert_eq!(
        (crossing.sampled_utc, crossing.direction),
        (8, Crossing::Below)
    );
    assert_eq!(crossing.thresholds, [Value::Watts(0), Value::Watts(10)]);
}

#[test]
fn energy_thresholds_and_terminal_utc_are_exact_at_integer_limits() {
    let energy = Channel {
        quantity: Quantity::ActiveEnergy(smgw::taf14::Direction::Import, None),
        ..POWER
    };
    let mut p = Profile::<1>::new(Config {
        thresholds: &[Value::WattHours(u64::MAX - 1)],
        ..Config::new("portal", energy, i64::MAX - 60)
    })
    .unwrap();
    assert_eq!(
        p.record(energy, i64::MAX - 60, Some(Value::WattHours(u64::MAX - 1))),
        Ok(None)
    );
    let final_block = p
        .record(energy, i64::MAX, Some(Value::WattHours(u64::MAX)))
        .unwrap()
        .unwrap();
    assert_eq!(final_block.crossing.unwrap().direction, Crossing::Above);
    assert_eq!(final_block.omitted, 1);
    assert_eq!(p.record(energy, i64::MIN, None), Err(Error::UnexpectedSlot));
}

#[test]
fn commissioning_rejects_configuration_that_cannot_be_honoured() {
    let config = Config::new("portal", POWER, 0);
    assert!(matches!(
        Profile::<0>::new(config),
        Err(Error::InvalidCapacity)
    ));
    for (invalid, error) in [
        (
            Config {
                profile_id: " ",
                ..config
            },
            Error::EmptyIdentifier,
        ),
        (
            Config {
                channel: Channel {
                    meter_id: "",
                    ..POWER
                },
                ..config
            },
            Error::EmptyIdentifier,
        ),
        (
            Config {
                reception_seconds: 0,
                ..config
            },
            Error::InvalidPeriod,
        ),
        (
            Config {
                delivery_every_slots: Some(0),
                ..config
            },
            Error::InvalidPeriod,
        ),
        (
            Config {
                delivery_every_slots: Some(3),
                ..config
            },
            Error::InvalidCapacity,
        ),
        (
            Config {
                thresholds: &[Value::WattHours(1)],
                ..config
            },
            Error::WrongUnit,
        ),
        (
            Config {
                thresholds: &[Value::Watts(1), Value::Watts(1)],
                ..config
            },
            Error::UnorderedThresholds,
        ),
        (
            Config {
                thresholds: &[Value::Watts(1), Value::Watts(-1)],
                ..config
            },
            Error::UnorderedThresholds,
        ),
    ] {
        assert_eq!(Profile::<2>::new(invalid).err(), Some(error));
    }
}
