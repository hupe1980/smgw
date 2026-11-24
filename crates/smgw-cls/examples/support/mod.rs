//! Single-association, read-only MMS peers for the loopback examples.
use std::{
    io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use iec61850_kit::{
    Error, Fc, Limits, ObjectReference, Result as MmsResult,
    common::monotonic_now,
    proto::{
        data::{DataView, Value},
        mms::{
            AccessResult, ConfirmedRequest, ConfirmedResponse, Mms, ObjectName, ServiceError,
            VariableAccess, VariableSpecification,
            association::{Association, AssociationEvent, CloseReason},
        },
    },
    server::{Acsi, Answer, Ied},
};
use smgw_cls::transport::{SmgwProxy, TlsEndpoint};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_openssl::SslStream;

pub(super) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const ROUTE: &str = "meter.test";
const HANDSHAKE: Duration = Duration::from_secs(5);
pub(super) const SESSION: Duration = Duration::from_secs(15);

/// Write pending MMS frames before returning each event, including release confirmation.
pub(super) async fn next_event(
    association: &mut Association,
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
) -> MmsResult<AssociationEvent> {
    let mut buffer = [0; 4096];
    loop {
        while let Some(packet) = association.poll_transmit() {
            stream.write_all(packet).await?;
        }
        stream.flush().await?;
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

pub(super) async fn serve_ied(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    ied: Ied,
) -> MmsResult<()> {
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
            _ => {
                return Err(Error::InvalidValue(
                    "unexpected event in read-only MMS server",
                ));
            }
        }
    }
}

pub(super) struct Reader<S> {
    association: Association,
    stream: S,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Reader<S> {
    pub(super) async fn fields(&mut self, fields: &[(String, Fc)]) -> MmsResult<Vec<Value>> {
        let names = fields
            .iter()
            .map(|(name, fc)| {
                let reference = ObjectReference::parse(name)?;
                let (domain, item) = reference.to_mms(*fc);
                Ok((domain.to_owned(), item))
            })
            .collect::<MmsResult<Vec<_>>>()?;
        let variables = names
            .iter()
            .map(|(domain, item)| {
                VariableSpecification::Name(ObjectName::DomainSpecific { domain, item })
            })
            .collect();
        let request = ConfirmedRequest::Read {
            specification_with_result: false,
            access: VariableAccess::ListOfVariable(variables),
        };
        let expected = self.association.call(monotonic_now(), &request)?;
        let AssociationEvent::Response { invoke_id, pdu } =
            next_event(&mut self.association, &mut self.stream).await?
        else {
            return Err(Error::InvalidValue("missing MMS Read response"));
        };
        if invoke_id != expected {
            return Err(Error::InvalidValue("unexpected MMS invoke ID"));
        }
        match Mms::parse(&pdu, &Limits::DEFAULT)? {
            Mms::ConfirmedResponse {
                service: ConfirmedResponse::Read { results, .. },
                ..
            } => {
                if results.len() != fields.len() {
                    return Err(Error::InvalidValue("wrong number of MMS Read results"));
                }
                results
                    .into_iter()
                    .map(|result| match result {
                        AccessResult::Success(tlv) => {
                            DataView::from_tlv(tlv)?.to_owned(&Limits::DEFAULT)
                        }
                        AccessResult::Failure(code) => Err(Error::DataAccess(code)),
                    })
                    .collect()
            }
            Mms::ConfirmedError { error, .. } => {
                let error = ServiceError::parse(&error)?;
                Err(Error::Service {
                    class: error.class,
                    code: error.code,
                })
            }
            _ => Err(Error::InvalidValue("expected a confirmed MMS Read")),
        }
    }
}

pub(super) async fn read_session<S: AsyncRead + AsyncWrite + Unpin, T>(
    stream: S,
    read: impl AsyncFnOnce(&mut Reader<S>) -> MmsResult<T>,
) -> MmsResult<T> {
    let mut reader = Reader {
        association: Association::client(Default::default()),
        stream,
    };
    reader.association.start(monotonic_now())?;
    if !matches!(
        next_event(&mut reader.association, &mut reader.stream).await?,
        AssociationEvent::Established(_)
    ) {
        return Err(Error::InvalidValue("MMS association setup failed"));
    }
    let value = read(&mut reader).await?;
    reader.association.release(monotonic_now())?;
    if !matches!(
        next_event(&mut reader.association, &mut reader.stream).await?,
        AssociationEvent::Closed(CloseReason::Released)
    ) {
        return Err(Error::InvalidValue("MMS release was not confirmed"));
    }
    reader.stream.shutdown().await?;
    Ok(value)
}

pub(super) async fn round_trip<T>(
    dir: &Path,
    ied: Ied,
    read: impl AsyncFnOnce(&mut Reader<SslStream<TcpStream>>) -> MmsResult<T>,
) -> Result<T> {
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
    let provider = async {
        let stream = cls.connect(gateway_addr, ROUTE, HANDSHAKE).await?;
        serve_ied(stream, ied).await.map_err(Into::into)
    };
    let relay = async {
        let (stream, _) = gateway.accept().await?;
        proxy.relay(stream).await?;
        Result::Ok(())
    };
    let reader = async {
        let (socket, _) = emt.accept().await?;
        read_session(aemt.accept(socket, HANDSHAKE).await?, read)
            .await
            .map_err(Into::into)
    };
    let (_, _, result) =
        timeout(SESSION, async { tokio::try_join!(provider, relay, reader) }).await??;
    Ok(result)
}

pub(super) fn pki_directory(name: &str, description: &str) -> Result<Option<PathBuf>> {
    let mut args = std::env::args_os().skip(1);
    let dir = args.next().unwrap_or_else(|| "target/test-pki".into());
    if args.next().is_some() {
        return Err(format!("usage: {name} [PKI_DIRECTORY]").into());
    }
    if dir == "--help" || dir == "-h" {
        println!(
            "{description}\n\nUsage: {name} [PKI_DIRECTORY]\nDefault: target/test-pki\nGenerate fixtures: cargo run -p xtask -- gen-pki"
        );
        return Ok(None);
    }
    let dir = PathBuf::from(dir);
    if !dir.is_dir() {
        return Err(format!("PKI directory {} is missing. Generate it with cargo run -p xtask -- gen-pki <DIRECTORY>.", dir.display()).into());
    }
    Ok(Some(dir))
}
