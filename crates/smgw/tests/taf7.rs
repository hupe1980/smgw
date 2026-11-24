use smgw::{
    taf2::{Reading, Registers, TariffRegister},
    taf7::*,
};
const METER: Channel<'static> = Channel {
    meter_id: "fixture",
    direction: Direction::Import,
};
fn sample(t: i64, wh: u64, quality: Quality) -> Observation {
    Observation::Received {
        source_utc: t,
        cumulative_wh: wh,
        quality,
    }
}

#[test]
fn refuses_misrouted_and_out_of_order_slots_atomically() {
    let mut block = Series::<2>::new(METER, Period::Hour, -3600).unwrap();
    let first = sample(-3601, 100, Quality::Valid);
    block.record(METER, -3600, first).unwrap();
    for (channel, time, error) in [
        (
            Channel {
                meter_id: "other",
                ..METER
            },
            0,
            Error::WrongChannel,
        ),
        (
            Channel {
                direction: Direction::Export,
                ..METER
            },
            0,
            Error::WrongChannel,
        ),
        (METER, 3600, Error::UnexpectedSlot),
        (METER, -3600, Error::UnexpectedSlot),
    ] {
        assert_eq!(
            block.record(channel, time, Observation::Missing),
            Err(error)
        );
        assert_eq!(block.next_slot(), Some(0));
        assert_eq!(
            block.entries(),
            &[Entry {
                scheduled_utc: -3600,
                observation: first
            }]
        );
    }
    block.record(METER, 0, Observation::Missing).unwrap();
    assert_eq!(block.channel(), METER);
    assert_eq!(block.period(), Period::Hour);
}

#[test]
fn retains_original_gap_and_quality_after_recovery() {
    let mut block = Series::<3>::new(METER, Period::QuarterHour, 0).unwrap();
    let original = [
        sample(-2, 1000, Quality::Valid),
        Observation::Missing,
        sample(1801, 1200, Quality::Valid),
    ];
    for (i, obs) in original.into_iter().enumerate() {
        block.record(METER, 900 * i as i64, obs).unwrap();
    }
    assert_eq!(block.next_slot(), None);
    assert_eq!(
        block.record(METER, 2700, Observation::Missing),
        Err(Error::Full)
    );
    assert!(block.entries().iter().map(|e| e.observation).eq(original));
    let mut regs = Registers::new(0, 1000).unwrap();
    for (entry, id) in block.entries()[1..].iter().zip([1, 2]) {
        regs.record(
            entry.scheduled_utc,
            TariffRegister::new(id).unwrap(),
            entry.observation.into(),
        )
        .unwrap();
    }
    assert_eq!((regs.total_wh(), regs.error_wh()), (200, 200));
    assert_eq!(block.entries()[1].observation, Observation::Missing);
}

#[test]
fn preserves_decreases_and_received_status() {
    let mut block = Series::<4>::new(METER, Period::QuarterHour, 900).unwrap();
    let mut regs = Registers::new(0, 101).unwrap();
    for (i, (quality, projected)) in [
        (Quality::Valid, Reading::Valid(100)),
        (Quality::TimeInvalid, Reading::TimeInvalid),
        (Quality::MeterFault, Reading::MeterFault(98)),
        (Quality::Fatal, Reading::Fatal),
    ]
    .into_iter()
    .enumerate()
    {
        let end = 900 * (i as i64 + 1);
        let obs = sample(end - 5, 100 - i as u64, quality);
        block.record(METER, end, obs).unwrap();
        assert_eq!(Reading::from(block.entries()[i].observation), projected);
        if i == 0 {
            assert_eq!(
                regs.record(end, TariffRegister::new(1).unwrap(), projected),
                Err(smgw::taf2::Error::DecreasingReading)
            );
        }
        assert_eq!(block.entries()[i].observation, obs);
    }
}

#[test]
fn construction_checks_grid_capacity_and_last_timestamp() {
    let p = Period::QuarterHour;
    assert!(matches!(
        Series::<0>::new(METER, p, 0),
        Err(Error::EmptyBlock)
    ));
    assert!(matches!(
        Series::<1>::new(
            Channel {
                meter_id: " ",
                ..METER
            },
            p,
            0
        ),
        Err(Error::EmptyMeterId)
    ));
    assert!(matches!(
        Series::<2>::new(METER, p, 1),
        Err(Error::UnalignedStart)
    ));
    let end = i64::MAX - i64::MAX % 900;
    assert!(matches!(
        Series::<2>::new(METER, p, end),
        Err(Error::TimestampOverflow)
    ));
    let mut block = Series::<1>::new(METER, p, end).unwrap();
    block.record(METER, end, Observation::Missing).unwrap();
    assert_eq!(block.next_slot(), None);
}
