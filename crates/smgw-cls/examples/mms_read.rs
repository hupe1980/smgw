//! Read the synthetic CLS measurement over two mutually authenticated loopback legs.
mod support;

use iec61850_kit::{Error, Fc, Result as MmsResult, proto::data::Value, server::Ied};
use support::{Reader, Result, pki_directory, round_trip};
use tokio::io::{AsyncRead, AsyncWrite};

fn meter() -> MmsResult<Ied> {
    Ied::from_scl(include_str!("fixtures/meter.cid"), Some("CLS"))
}

async fn read_meter<S: AsyncRead + AsyncWrite + Unpin>(reader: &mut Reader<S>) -> MmsResult<f32> {
    let values = reader
        .fields(&[("CLSLD0/MMXU1.TotW.mag.f".into(), Fc::MX)])
        .await?;
    match values.as_slice() {
        [Value::Float32(watts)] => Ok(*watts),
        _ => Err(Error::InvalidValue("measurement has an unexpected type")),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let Some(dir) = pki_directory(
        "mms_read",
        "Read the synthetic CLS meter through a loopback gateway.",
    )?
    else {
        return Ok(());
    };
    let watts = round_trip(&dir, meter()?, read_meter).await?;
    if watts != 1250.0 {
        return Err(format!("expected 1250 W in meter.cid, received {watts} W").into());
    }
    println!("CLSLD0/MMXU1$MX$TotW$mag$f = {watts} W (MMS read over HAN and WAN mutual TLS)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iec61850_kit::{
        Limits,
        common::monotonic_now,
        proto::mms::{
            ConfirmedRequest, Mms,
            association::{Association, AssociationEvent, CloseReason},
        },
    };
    use support::{SESSION, next_event, read_session, serve_ied};
    use tokio::time::timeout;

    #[tokio::test]
    async fn read_named_scl_measurement() -> Result<()> {
        // Fragment every MMS frame through a tiny transport; the same driver
        // and device model are used by the TLS example.
        let (server, client) = tokio::io::duplex(31);
        let (_, watts) = timeout(SESSION, async {
            tokio::try_join!(
                serve_ied(server, meter()?),
                read_session(client, read_meter)
            )
        })
        .await??;
        assert_eq!(watts, 1250.0);
        Ok(())
    }

    #[tokio::test]
    async fn reject_services_outside_read_only_profile() -> Result<()> {
        use iec61850_kit::server::{DATA_ACCESS_DENIED, error_class};
        let (server, mut peer) = tokio::io::duplex(128);
        let client = async {
            let mut assoc = Association::client(Default::default());
            assoc.start(monotonic_now())?;
            assert!(matches!(
                next_event(&mut assoc, &mut peer).await?,
                AssociationEvent::Established(_)
            ));
            let id = assoc.call(monotonic_now(), &ConfirmedRequest::Identify)?;
            let AssociationEvent::Response { pdu, invoke_id } =
                next_event(&mut assoc, &mut peer).await?
            else {
                return Err("expected a service denial".into());
            };
            assert_eq!(invoke_id, id);
            let Mms::ConfirmedError { error, .. } = Mms::parse(&pdu, &Limits::DEFAULT)? else {
                return Err("non-read request was accepted".into());
            };
            let error = iec61850_kit::proto::mms::ServiceError::parse(&error)?;
            assert_eq!(
                (error.class, error.code),
                (error_class::ACCESS, DATA_ACCESS_DENIED)
            );
            assoc.release(monotonic_now())?;
            assert!(matches!(
                next_event(&mut assoc, &mut peer).await?,
                AssociationEvent::Closed(CloseReason::Released)
            ));
            Result::Ok(())
        };
        timeout(SESSION, async {
            tokio::try_join!(
                async { serve_ied(server, meter()?).await.map_err(Into::into) },
                client
            )
        })
        .await??;
        Ok(())
    }
}
