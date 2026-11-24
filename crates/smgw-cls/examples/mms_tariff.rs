//! Import a synthetic daily FSCH schedule through the authenticated loopback gateway.
mod support;

use iec61850_kit::{
    Error, Result as MmsResult, TimeQuality, UtcTime, proto::data::Value, server::Ied,
};
use smgw_cls::fnn::{
    fsch::{TariffProfile, TariffRead, read_tariff_windows_async},
    tariff::{TariffLevel, TariffWindow},
};
use support::{Reader, Result, pki_directory, round_trip};
use tokio::io::{AsyncRead, AsyncWrite};

const DAY: std::ops::Range<u32> = 86_400..172_800;

fn provider(quality: TimeQuality) -> MmsResult<Ied> {
    let mut ied = Ied::from_scl(include_str!("fixtures/tariff.cid"), Some("CLS"))?;
    ied.set_internal("CLSLD0/FSCH1$CF$SchdIntv$units$SIUnit", Value::Integer(84))
        .map_err(Error::DataAccess)?;
    ied.set_internal(
        "CLSLD0/FSCH1$SP$StrTm1$setTm",
        Value::UtcTime(UtcTime::from_unix(DAY.start, 0, quality)),
    )
    .map_err(Error::DataAccess)?;
    Ok(ied)
}

async fn read_day<S: AsyncRead + AsyncWrite + Unpin>(
    reader: &mut Reader<S>,
) -> MmsResult<Vec<TariffWindow>> {
    let profile = TariffProfile {
        first_price: "ValING1",
        ..TariffProfile::new("ct/kWh")
    };
    let read = TariffRead::new("CLSLD0/FSCH1", profile, DAY)?;
    read_tariff_windows_async(
        read,
        async |fields| reader.fields(fields).await,
        |t| !t.quality.clock_failure && !t.quality.clock_not_synchronized,
        |price| match price {
            12.0 => Some(TariffLevel::Standard),
            6.0 => Some(TariffLevel::Low),
            18.0 => Some(TariffLevel::High),
            _ => None,
        },
    )
    .await
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let Some(dir) = pki_directory(
        "mms_tariff",
        "Read the synthetic daily FSCH tariff through a loopback gateway.",
    )?
    else {
        return Ok(());
    };
    let windows = round_trip(&dir, provider(TimeQuality::SYNCHRONIZED)?, read_day).await?;
    println!("UTC interval (Unix seconds)  Tariff level");
    for w in windows {
        println!("[{}, {})  {:?}", w.start_utc, w.end_utc, w.level);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iec61850_kit::{Fc, server::DATA_ACCESS_NON_EXISTENT};
    use support::{SESSION, read_session, serve_ied};
    use tokio::{
        io::{BufStream, duplex},
        time::timeout,
    };

    #[tokio::test]
    async fn fragmented_tariff_covers_the_delivery_day() -> Result<()> {
        let (cls, emt) = duplex(7);
        let (_, windows) = timeout(SESSION, async {
            tokio::try_join!(
                serve_ied(BufStream::new(cls), provider(TimeQuality::SYNCHRONIZED)?),
                read_session(BufStream::new(emt), read_day)
            )
        })
        .await??;
        assert_eq!(
            windows,
            [
                TariffWindow {
                    start_utc: 86_400,
                    end_utc: 115_200,
                    level: TariffLevel::Standard
                },
                TariffWindow {
                    start_utc: 115_200,
                    end_utc: 144_000,
                    level: TariffLevel::Low
                },
                TariffWindow {
                    start_utc: 144_000,
                    end_utc: 172_800,
                    level: TariffLevel::High
                },
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn unsynchronized_provider_fails_the_import() -> Result<()> {
        let (cls, emt) = duplex(31);
        let ied = provider(TimeQuality::UNSYNCHRONIZED)?;
        let (server, client) = timeout(SESSION, async {
            tokio::join!(serve_ied(cls, ied), read_session(emt, read_day))
        })
        .await?;
        assert_eq!(
            client,
            Err(Error::InvalidValue("rejected FSCH start timestamp"))
        );
        assert!(server.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn missing_field_preserves_mms_access_error() -> Result<()> {
        let (cls, emt) = duplex(31);
        let ied = provider(TimeQuality::SYNCHRONIZED)?;
        let (_, client) = timeout(SESSION, async {
            tokio::join!(
                serve_ied(cls, ied),
                read_session(emt, async |reader| {
                    reader
                        .fields(&[
                            ("CLSLD0/FSCH1.ValING1.setVal".into(), Fc::SP),
                            ("CLSLD0/FSCH1.ValING4.setVal".into(), Fc::SP),
                        ])
                        .await
                })
            )
        })
        .await?;
        assert_eq!(client, Err(Error::DataAccess(DATA_ACCESS_NON_EXISTENT)));
        Ok(())
    }
}
