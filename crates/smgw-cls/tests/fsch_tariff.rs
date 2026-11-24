// The loopback peer uses the simulator's server feature.
#![cfg(all(feature = "fsch-tariff", feature = "simulator"))]

use iec61850_kit::{
    Error, Fc, ObjectReference, TimeQuality, UtcTime,
    client::Client,
    proto::data::Value,
    server::{Ied, Server},
};
use smgw_cls::fnn::{
    fsch::{TariffProfile, TariffRead, read_tariff_windows, read_tariff_windows_async},
    tariff::{TariffLevel, TariffSchedule},
};

const NODE: &str = "CLSLD0/FSCH1";
const START_FIELD: &str = "CLSLD0/FSCH1$SP$StrTm1$setTm";
const DAY: std::ops::Range<u32> = 86_400..172_800;
const PROFILE: TariffProfile<'_> = TariffProfile {
    first_price: "ValING1",
    ..TariffProfile::new("ct/kWh")
};

fn fixture(quality: TimeQuality) -> Ied {
    let mut ied =
        Ied::from_scl(include_str!("../examples/fixtures/tariff.cid"), Some("CLS")).unwrap();
    ied.set_internal("CLSLD0/FSCH1$CF$SchdIntv$units$SIUnit", Value::Integer(84))
        .unwrap();
    ied.set_internal(
        START_FIELD,
        Value::UtcTime(UtcTime::from_unix(DAY.start, 0, quality)),
    )
    .unwrap();
    ied
}

fn classify(price: f64) -> Option<TariffLevel> {
    match price {
        6.0 => Some(TariffLevel::Low),
        12.0 => Some(TariffLevel::Standard),
        18.0 => Some(TariffLevel::High),
        _ => None,
    }
}

fn read_fields(ied: &Ied, fields: &[(String, Fc)]) -> iec61850_kit::Result<Vec<Value>> {
    fields
        .iter()
        .map(|(name, fc)| {
            let reference = ObjectReference::parse(name)?;
            let (domain, item) = reference.to_mms(*fc);
            ied.read(domain, &item)
                .ok_or(Error::InvalidValue("missing fixture field"))
        })
        .collect()
}

#[tokio::test]
async fn both_readers_apply_clock_policy_before_classifying()
-> Result<(), Box<dyn std::error::Error>> {
    for quality in [
        TimeQuality {
            accuracy: 12,
            ..TimeQuality::SYNCHRONIZED
        },
        TimeQuality::UNSYNCHRONIZED,
        TimeQuality {
            clock_failure: true,
            ..TimeQuality::SYNCHRONIZED
        },
    ] {
        let server = Server::bind("127.0.0.1:0", fixture(quality))?;
        let addr = server.local_addr()?;
        let peer = std::thread::spawn(move || server.accept_one());
        let mut client = Client::connect(&addr.to_string())?;
        let accept_start = |start: UtcTime| {
            assert_eq!(start, UtcTime::from_unix(DAY.start, 0, quality));
            !start.quality.clock_not_synchronized && !start.quality.clock_failure
        };
        let checked_price = |price| {
            assert!(!quality.clock_failure && !quality.clock_not_synchronized);
            classify(price)
        };
        let blocking =
            read_tariff_windows(&mut client, NODE, PROFILE, DAY, accept_start, checked_price);
        let ied = fixture(quality);
        let asynchronous = read_tariff_windows_async(
            TariffRead::new(NODE, PROFILE, DAY)?.with_batch_limit(2),
            async |fields| read_fields(&ied, fields),
            accept_start,
            checked_price,
        )
        .await;
        assert_eq!(blocking, asynchronous);
        if quality.clock_failure || quality.clock_not_synchronized {
            assert_eq!(
                blocking,
                Err(Error::InvalidValue("rejected FSCH start timestamp"))
            );
        } else {
            let windows = blocking?;
            let schedule = TariffSchedule::new(&windows)?;
            assert_eq!(
                schedule.active_at(DAY.start.into()).unwrap().level,
                TariffLevel::Standard
            );
            assert_eq!(schedule.active_at(115200).unwrap().level, TariffLevel::Low);
            assert_eq!(schedule.active_at(172799).unwrap().level, TariffLevel::High);
            assert_eq!(schedule.active_at(DAY.end.into()), None);
        }
        assert_eq!(
            read_tariff_windows(
                &mut client,
                NODE,
                PROFILE,
                DAY,
                |_| true,
                |price| (price != 18.0).then_some(TariffLevel::Standard)
            ),
            Err(Error::InvalidValue("unmapped FSCH tariff price"))
        );
        client.release()?;
        peer.join().unwrap()?;
    }
    Ok(())
}

#[tokio::test]
async fn changed_clock_quality_in_final_header_rejects_the_import() {
    let mut ied = fixture(TimeQuality::SYNCHRONIZED);
    let read = TariffRead::new(NODE, PROFILE, DAY).unwrap();
    let result = read_tariff_windows_async(
        read,
        async |fields| {
            let values = read_fields(&ied, fields)?;
            if fields.iter().any(|(field, _)| field.contains(".ValING")) {
                ied.set_internal(
                    START_FIELD,
                    Value::UtcTime(UtcTime::from_unix(
                        DAY.start,
                        0,
                        TimeQuality::UNSYNCHRONIZED,
                    )),
                )
                .unwrap();
            }
            Ok(values)
        },
        |_| panic!("unverified timestamp reached policy"),
        |_| panic!("unverified price"),
    )
    .await;
    assert_eq!(
        result,
        Err(Error::InvalidValue(
            "FSCH header changed while reading prices"
        ))
    );
}

#[tokio::test]
async fn cancelling_while_waiting_for_the_closing_header_discards_all_prices() {
    let ied = fixture(TimeQuality::SYNCHRONIZED);
    let mut received_prices = false;
    let mut waiting_for_header = false;
    let import = read_tariff_windows_async(
        TariffRead::new(NODE, PROFILE, DAY).unwrap(),
        async |fields| {
            if received_prices {
                assert!(
                    fields
                        .iter()
                        .any(|(field, _)| field.ends_with(".StrTm1.setTm"))
                );
                waiting_for_header = true;
                return std::future::pending().await;
            }
            received_prices = fields.iter().any(|(field, _)| field.contains(".ValING"));
            read_fields(&ied, fields)
        },
        |_| panic!("clock policy called before header confirmation"),
        |_| panic!("partial prices reached classification"),
    );
    tokio::select! {
        biased;
        result = import => panic!("unfinished import resolved: {result:?}"),
        () = std::future::ready(()) => {}
    }
    assert!(waiting_for_header);
}
