use smgw::taf9::{ActivePower, Channel, Crossing, Error, Profile};

const POWER: Channel<'static> = Channel {
    meter_id: "meter",
    quantity: ActivePower::Total,
};

#[test]
fn detects_both_crossings_and_keeps_the_periodic_trigger() {
    let mut p = Profile::new("p", POWER, 0, true, Some(-2000)).unwrap();
    for (i, (watts, expected)) in [
        (-2000, None),
        (-2100, Some(Crossing::Below)),
        (-2200, None),
        (-2000, None),
        (-1900, Some(Crossing::Above)),
    ]
    .into_iter()
    .enumerate()
    {
        let utc = i as i64 * 60;
        let event = p.record(POWER, utc, Some(watts)).unwrap().unwrap();
        assert!(event.periodic);
        assert_eq!(
            (
                event.crossing,
                event.reading.profile_id,
                event.reading.channel,
                event.reading.watts
            ),
            (expected, "p", POWER, watts)
        );
        assert_eq!(p.on_demand(utc + 59), Some(event.reading));
        assert_eq!(p.on_demand(utc + 60), None);
    }
}

#[test]
fn gaps_and_other_channels_cannot_generate_crossings() {
    let mut p = Profile::new("p", POWER, -60, false, Some(0)).unwrap();
    assert_eq!(p.record(POWER, -60, Some(-1)).unwrap(), None);
    let other = Channel {
        quantity: ActivePower::L1,
        ..POWER
    };
    assert_eq!(p.record(other, 0, Some(8)), Err(Error::WrongChannel));
    assert_eq!(p.record(POWER, 60, Some(8)), Err(Error::UnexpectedSlot));
    assert_eq!(p.record(POWER, 0, None).unwrap(), None);
    assert_eq!(p.on_demand(0), None);
    assert_eq!(p.record(POWER, 60, Some(1)).unwrap(), None);
    assert_eq!(p.on_demand(59), None);
    assert_eq!(p.on_demand(60).unwrap().watts, 1);
    let event = p.record(POWER, 120, Some(-1)).unwrap().unwrap();
    assert!(!event.periodic);
    assert_eq!(event.crossing, Some(Crossing::Below));
}

#[test]
fn extreme_timestamps_and_table_4_7_quantities() {
    let mut p = Profile::new("p", POWER, i64::MAX, false, None).unwrap();
    assert!(p.record(POWER, i64::MAX, Some(0)).unwrap().is_none());
    assert_eq!(p.on_demand(i64::MIN), None);
    assert_eq!(p.on_demand(i64::MAX).unwrap().watts, 0);
    assert_eq!(p.record(POWER, i64::MIN, None), Err(Error::UnexpectedSlot));
    assert_eq!(
        [
            ActivePower::L1,
            ActivePower::L2,
            ActivePower::L3,
            ActivePower::Total
        ]
        .map(ActivePower::obis),
        [
            "1-0:36.7.0.255",
            "1-0:56.7.0.255",
            "1-0:76.7.0.255",
            "1-0:16.7.0.255"
        ]
    );
}
