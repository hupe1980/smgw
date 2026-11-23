//! Read the synthetic CLS measurement over two mutually authenticated loopback legs.
use std::{error::Error, io, net::Ipv4Addr, path::Path, time::Duration};

use iec61850_kit::{
    Limits,
    common::monotonic_now,
    proto::{
        data::DataView,
        mms::{
            AccessResult, ConfirmedRequest, ConfirmedResponse, Mms, ObjectName, VariableAccess,
            VariableSpecification,
            association::{Association, AssociationEvent, CloseReason},
        },
    },
    server::{Acsi, Answer, Ied},
};
use smgw_cls::transport::{SmgwProxy, TlsEndpoint};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const DOMAIN: &str = "CLSLD0";
const ITEM: &str = "MMXU1$MX$TotW$mag$f";
const ROUTE: &str = "meter.test";
const HANDSHAKE: Duration = Duration::from_secs(5);
const SESSION: Duration = Duration::from_secs(15);

/// Write pending MMS frames before returning each event, including release confirmation.
async fn next_event(
    association: &mut Association,
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
) -> Result<AssociationEvent> {
    let mut buffer = [0; 4096];
    loop {
        while let Some(packet) = association.poll_transmit() {
            stream.write_all(packet).await?;
        }
        if let Some(event) = association.poll_event() {
            return Ok(event);
        }
        let read = stream.read(&mut buffer);
        let count = match association.next_timeout() {
            Some(deadline) => {
                let wait = Duration::from_nanos(deadline.nanos_since(monotonic_now()));
                match timeout(wait, read).await {
                    Ok(result) => result?,
                    Err(_) => {
                        association.on_timeout(monotonic_now());
                        continue;
                    }
                }
            }
            None => read.await?,
        };
        if count == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "MMS stream closed").into());
        }
        association.on_bytes(monotonic_now(), &buffer[..count]);
    }
}

async fn serve_meter(mut stream: impl AsyncRead + AsyncWrite + Unpin) -> Result<()> {
    let ied = Ied::from_scl(include_str!("fixtures/meter.cid"), Some("CLS"))?;
    let mut acsi = Acsi::new(ied);
    let mut association = Association::server(Default::default());
    // This private model belongs to exactly one association. Dropping the model
    // also releases all of its state on I/O errors and task cancellation.
    const ID: u64 = 1;
    loop {
        match next_event(&mut association, &mut stream).await? {
            AssociationEvent::Established(parameters) => {
                acsi.on_association_opened(ID, Vec::new());
                acsi.set_association_max_pdu(ID, parameters.max_pdu);
                acsi.set_authenticated(ID, parameters.authenticated);
            }
            AssociationEvent::Request { invoke_id, pdu } => {
                let answer = match Mms::parse(&pdu, &Limits::DEFAULT)? {
                    Mms::ConfirmedRequest { ref service, .. }
                        if matches!(service, ConfirmedRequest::Read { .. }) =>
                    {
                        acsi.request(ID, monotonic_now(), service)
                    }
                    Mms::ConfirmedRequest { .. } => Answer::DENIED,
                    _ => Answer::INVALID_PDU,
                };
                association.send_encoded(&answer.encode(invoke_id)?)?;
            }
            AssociationEvent::Closed(CloseReason::PeerReleased) => {
                acsi.on_association_closed(ID, monotonic_now());
                stream.shutdown().await?;
                return Ok(());
            }
            _ => return Err("unexpected event in read-only MMS server".into()),
        }
    }
}

async fn read_meter(mut stream: impl AsyncRead + AsyncWrite + Unpin) -> Result<f32> {
    let mut association = Association::client(Default::default());
    association.start(monotonic_now())?;
    if !matches!(
        next_event(&mut association, &mut stream).await?,
        AssociationEvent::Established(_)
    ) {
        return Err("MMS association setup failed".into());
    }
    let name = ObjectName::DomainSpecific {
        domain: DOMAIN,
        item: ITEM,
    };
    let request = ConfirmedRequest::Read {
        specification_with_result: false,
        access: VariableAccess::ListOfVariable(vec![VariableSpecification::Name(name)]),
    };
    let expected_id = association.call(monotonic_now(), &request)?;
    let AssociationEvent::Response { invoke_id, pdu } =
        next_event(&mut association, &mut stream).await?
    else {
        return Err("missing MMS Read response".into());
    };
    if invoke_id != expected_id {
        return Err("response has a different invoke ID".into());
    }
    let Mms::ConfirmedResponse {
        service: ConfirmedResponse::Read { results, .. },
        ..
    } = Mms::parse(&pdu, &Limits::DEFAULT)?
    else {
        return Err("expected a confirmed Read".into());
    };
    let [AccessResult::Success(value)] = results.as_slice() else {
        return Err("measurement unavailable".into());
    };
    let DataView::Float32(watts) = DataView::from_tlv(*value)? else {
        return Err("measurement has an unexpected type".into());
    };
    association.release(monotonic_now())?;
    if !matches!(
        next_event(&mut association, &mut stream).await?,
        AssociationEvent::Closed(CloseReason::Released)
    ) {
        return Err("MMS release was not confirmed".into());
    }
    stream.shutdown().await?;
    Ok(watts)
}

async fn round_trip(dir: &Path) -> Result<f32> {
    let gateway = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let emt = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let cls = TlsEndpoint::load(dir, "han", "cls", "smgw")?;
    let aemt = TlsEndpoint::load(dir, "wan", "aemt", "smgw")?;
    let mut proxy = SmgwProxy::new(
        TlsEndpoint::load(dir, "han", "smgw", "cls")?,
        TlsEndpoint::load(dir, "wan", "smgw", "aemt")?,
        ROUTE.into(),
        emt.local_addr()?,
        "aemt.test".into(),
        HANDSHAKE,
        SESSION,
    )?;
    let gateway_addr = gateway.local_addr()?;
    let meter = async {
        let stream = cls.connect(gateway_addr, ROUTE, HANDSHAKE).await?;
        serve_meter(stream).await
    };
    let relay = async {
        let (stream, _) = gateway.accept().await?;
        proxy.relay(stream).await?;
        Result::Ok(())
    };
    let reader = async {
        let (socket, _) = emt.accept().await?;
        read_meter(aemt.accept(socket, HANDSHAKE).await?).await
    };
    let (_, _, watts) =
        timeout(SESSION, async { tokio::try_join!(meter, relay, reader) }).await??;
    Ok(watts)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let dir = args.next().unwrap_or_else(|| "target/test-pki".into());
    if args.next().is_some() {
        return Err("usage: mms_read [PKI_DIRECTORY]".into());
    }
    if dir == "--help" || dir == "-h" {
        println!(
            "Read the synthetic CLS meter through a loopback gateway.\n\nUsage: mms_read [PKI_DIRECTORY]\nDefault: target/test-pki\nGenerate fixtures: cargo run -p xtask -- gen-pki"
        );
        return Ok(());
    }
    let dir = Path::new(&dir);
    if !dir.is_dir() {
        return Err(format!(
            "PKI directory {} is missing. Generate it with cargo run -p xtask -- gen-pki <DIRECTORY>.",
            dir.display()
        ).into());
    }
    let watts = round_trip(dir).await?;
    if watts != 1250.0 {
        return Err(format!("expected 1250 W in meter.cid, received {watts} W").into());
    }
    println!("{DOMAIN}/{ITEM} = {watts} W (MMS read over HAN and WAN mutual TLS)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_named_scl_measurement() -> Result<()> {
        // Fragment every MMS frame through a tiny transport; the same driver
        // and device model are used by the TLS example.
        let (server, client) = tokio::io::duplex(31);
        let (_, watts) = timeout(SESSION, async {
            tokio::try_join!(serve_meter(server), read_meter(client))
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
            tokio::try_join!(serve_meter(server), client)
        })
        .await??;
        Ok(())
    }
}
