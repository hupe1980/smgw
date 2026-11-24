use smgw::{
    taf1::{ClosingReading, Error, Profile},
    taf7::{Channel, Direction, Entry, Observation, Quality},
};

const CHANNEL: Channel = Channel {
    meter_id: "meter-1",
    direction: Direction::Import,
};
const FEB: i64 = 1_769_904_000; // 2026-02-01 00:00 UTC
const MAR: i64 = 1_772_323_200;
const APR: i64 = 1_775_001_600;

fn sample(end: i64, cumulative_wh: u64) -> Entry {
    Entry {
        scheduled_utc: end,
        observation: Observation::Received {
            source_utc: end - 1,
            cumulative_wh,
            quality: Quality::Valid,
        },
    }
}

#[test]
fn rejects_bad_commissioning() {
    assert!(matches!(
        Profile::new(" ", CHANNEL, &[FEB]),
        Err(Error::EmptyProfileId)
    ));
    assert!(matches!(
        Profile::new(
            "p",
            Channel {
                meter_id: "",
                ..CHANNEL
            },
            &[FEB]
        ),
        Err(Error::EmptyMeterId)
    ));
    for closes in [&[][..], &[MAR, FEB], &[FEB, FEB]] {
        assert!(matches!(
            Profile::new("p", CHANNEL, closes),
            Err(Error::InvalidSchedule)
        ));
    }
}

#[test]
fn refuses_wrong_channel_and_instant_without_advancing() {
    let mut profile = Profile::new("monthly", CHANNEL, &[FEB, MAR]).unwrap();
    for other in [
        Channel {
            meter_id: "meter-2",
            ..CHANNEL
        },
        Channel {
            direction: Direction::Export,
            ..CHANNEL
        },
    ] {
        assert_eq!(
            profile.close(other, sample(FEB, 42)),
            Err(Error::WrongChannel)
        );
    }
    for end in [FEB - 900, MAR] {
        assert_eq!(
            profile.close(CHANNEL, sample(end, 42)),
            Err(Error::UnexpectedClose)
        );
    }
    assert_eq!(profile.next_close(), Some(FEB));
    profile.close(CHANNEL, sample(FEB, 512000)).unwrap();
    assert_eq!(
        profile.close(CHANNEL, sample(FEB, 512000)),
        Err(Error::UnexpectedClose)
    );
    assert_eq!(profile.next_close(), Some(MAR));
}

#[test]
fn emits_only_the_cumulative_close_and_skips_unavailable_periods() {
    for bad in [
        Observation::Missing,
        Observation::Received {
            source_utc: FEB,
            cumulative_wh: 12345,
            quality: Quality::TimeInvalid,
        },
        Observation::Received {
            source_utc: MAR,
            cumulative_wh: 12345,
            quality: Quality::MeterFault,
        },
        Observation::Received {
            source_utc: MAR,
            cumulative_wh: 12345,
            quality: Quality::Fatal,
        },
    ] {
        let mut profile = Profile::new("monthly", CHANNEL, &[FEB, MAR, APR]).unwrap();
        assert_eq!(
            profile
                .close(CHANNEL, sample(FEB, 512000))
                .unwrap()
                .unwrap()
                .cumulative_wh,
            512000
        );
        assert_eq!(
            profile.close(
                CHANNEL,
                Entry {
                    scheduled_utc: MAR,
                    observation: bad
                }
            ),
            Ok(None)
        );
        let ClosingReading {
            profile_id,
            channel,
            end_utc,
            cumulative_wh,
        } = profile
            .close(CHANNEL, sample(APR, 567000))
            .unwrap()
            .unwrap();
        assert_eq!(
            (profile_id, channel, end_utc, cumulative_wh),
            ("monthly", CHANNEL, APR, 567000)
        );
        assert_eq!(profile.next_close(), None);
        assert_eq!(
            profile.close(CHANNEL, sample(APR, 567000)),
            Err(Error::ScheduleComplete)
        );
    }
}
