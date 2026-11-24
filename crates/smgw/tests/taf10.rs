use smgw::taf10::{
    ActivePower, Channel, Config, Crossing, Error, Phase, PhaseAngle, Profile, Quantity, Reading,
    Value,
};

const VOLTAGE: Channel<'static> = Channel {
    meter_id: "physical-meter",
    quantity: Quantity::Voltage(Phase::L1),
};

fn config() -> Config<'static> {
    Config {
        profile_id: "grid",
        channel: VOLTAGE,
        pseudonym: Some("feeder-17"),
        first_utc: 0,
        periodic: true,
        threshold: None,
    }
}

#[test]
fn table_4_9_channels_accept_only_their_own_unit() {
    use Phase::{L1, L2, L3};
    use Quantity::{Current, Frequency, Voltage};
    let w = Value::Watts(-12);
    let ma = Value::Milliamps(123);
    let mhz = Value::Millihertz(50_010);
    let angle = Value::Millidegrees(-24_000);
    let mv = Value::Millivolts(230_000);
    for (quantity, expected_obis, sample) in [
        (Quantity::ActivePower(ActivePower::L1), "1-0:36.7.0.255", w),
        (Quantity::ActivePower(ActivePower::L2), "1-0:56.7.0.255", w),
        (Quantity::ActivePower(ActivePower::L3), "1-0:76.7.0.255", w),
        (
            Quantity::ActivePower(ActivePower::Total),
            "1-0:16.7.0.255",
            w,
        ),
        (Current(L1), "1-0:31.7.0.255", ma),
        (Current(L2), "1-0:51.7.0.255", ma),
        (Current(L3), "1-0:71.7.0.255", ma),
        (Frequency, "1-0:14.7.0.255", mhz),
        (
            Quantity::PhaseAngle(PhaseAngle::VoltageL2ToL1),
            "1-0:81.7.1.255",
            angle,
        ),
        (
            Quantity::PhaseAngle(PhaseAngle::VoltageL3ToL1),
            "1-0:81.7.2.255",
            angle,
        ),
        (
            Quantity::PhaseAngle(PhaseAngle::CurrentToVoltage(L1)),
            "1-0:81.7.4.255",
            angle,
        ),
        (
            Quantity::PhaseAngle(PhaseAngle::CurrentToVoltage(L2)),
            "1-0:81.7.15.255",
            angle,
        ),
        (
            Quantity::PhaseAngle(PhaseAngle::CurrentToVoltage(L3)),
            "1-0:81.7.26.255",
            angle,
        ),
        (Voltage(L1), "1-0:32.7.0.255", mv),
        (Voltage(L2), "1-0:52.7.0.255", mv),
        (Voltage(L3), "1-0:72.7.0.255", mv),
    ] {
        assert_eq!(quantity.obis(), expected_obis);
        let channel = Channel {
            quantity,
            ..VOLTAGE
        };
        let mut profile = Profile::new(Config {
            channel,
            threshold: Some(sample),
            ..config()
        })
        .unwrap();
        for other in [w, ma, mhz, angle, mv].into_iter().filter(|&v| v != sample) {
            assert!(matches!(
                Profile::new(Config {
                    channel,
                    threshold: Some(other),
                    ..config()
                }),
                Err(Error::WrongUnit)
            ));
            assert_eq!(
                profile.record(channel, 0, Some(other)),
                Err(Error::WrongUnit)
            );
        }
        let reading = profile
            .record(channel, 0, Some(sample))
            .unwrap()
            .unwrap()
            .reading;
        assert_eq!((reading.quantity, reading.value), (quantity, sample));
    }
}

// TAF 10 SR-001 covers all three reporting paths.
#[test]
fn every_output_uses_the_commissioned_reference() {
    for periodic in [false, true] {
        for pseudonym in [None, Some("feeder-17")] {
            let mut profile = Profile::new(Config {
                periodic,
                pseudonym,
                threshold: Some(Value::Millivolts(240_000)),
                ..config()
            })
            .unwrap();
            for (utc, mv, crossing) in [
                (0, 240_000, None),
                (60, 241_000, Some(Crossing::Above)),
                (120, 239_000, Some(Crossing::Below)),
            ] {
                let value = Value::Millivolts(mv);
                let expected = Reading {
                    profile_id: "grid",
                    meter_reference: pseudonym.unwrap_or(VOLTAGE.meter_id),
                    quantity: VOLTAGE.quantity,
                    sampled_utc: utc,
                    value,
                };
                let notification = profile.record(VOLTAGE, utc, Some(value)).unwrap();
                if periodic || crossing.is_some() {
                    let notification = notification.unwrap();
                    assert_eq!(notification.reading, expected);
                    assert_eq!(
                        (notification.periodic, notification.crossing),
                        (periodic, crossing)
                    );
                } else {
                    assert_eq!(notification, None);
                }
                assert_eq!(profile.on_demand(utc + 59), Some(expected));
                assert_eq!(profile.on_demand(utc + 60), None);
            }
        }
    }
}

// TAF 10 SR-002 covers source binding and freshness.
#[test]
fn rejected_samples_preserve_the_slot_and_gaps_break_the_baseline() {
    let mut profile = Profile::new(Config {
        periodic: false,
        threshold: Some(Value::Millivolts(230_000)),
        ..config()
    })
    .unwrap();
    let low = Value::Millivolts(225_000);
    let high = Some(Value::Millivolts(235_000));
    assert_eq!(profile.record(VOLTAGE, 0, Some(low)), Ok(None));
    let first = profile.on_demand(0).unwrap();
    for channel in [
        Channel {
            meter_id: "other",
            ..VOLTAGE
        },
        Channel {
            meter_id: "feeder-17",
            ..VOLTAGE
        },
        Channel {
            quantity: Quantity::Voltage(Phase::L2),
            ..VOLTAGE
        },
    ] {
        assert_eq!(profile.record(channel, 60, None), Err(Error::WrongChannel));
    }
    assert_eq!(
        profile.record(VOLTAGE, 60, Some(Value::Millihertz(235_000))),
        Err(Error::WrongUnit)
    );
    assert_eq!(
        profile.record(VOLTAGE, 120, high),
        Err(Error::UnexpectedSlot)
    );
    assert_eq!(profile.record(VOLTAGE, 0, None), Err(Error::UnexpectedSlot));
    assert_eq!(profile.on_demand(59), Some(first));
    assert_eq!(profile.on_demand(60), None);
    assert_eq!(
        profile.record(VOLTAGE, 60, high).unwrap().unwrap().crossing,
        Some(Crossing::Above)
    );
    assert_eq!(profile.record(VOLTAGE, 120, None), Ok(None));
    assert_eq!(profile.on_demand(120), None);
    assert_eq!(profile.record(VOLTAGE, 180, Some(low)), Ok(None));
    assert_eq!(profile.on_demand(179), None);
    assert_eq!(profile.on_demand(180).unwrap().value, low);
}

#[test]
fn commissioning_validates_outgoing_identifiers() {
    for invalid in [
        Config {
            profile_id: " ",
            ..config()
        },
        Config {
            channel: Channel {
                meter_id: "",
                ..VOLTAGE
            },
            ..config()
        },
        Config {
            pseudonym: Some(" "),
            ..config()
        },
    ] {
        assert!(matches!(Profile::new(invalid), Err(Error::EmptyIdentifier)));
    }
    for identifying in [
        Config {
            pseudonym: Some(" physical-meter "),
            ..config()
        },
        Config {
            profile_id: VOLTAGE.meter_id,
            ..config()
        },
    ] {
        assert!(matches!(
            Profile::new(identifying),
            Err(Error::IdentifyingReport)
        ));
    }
}
