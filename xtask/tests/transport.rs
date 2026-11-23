//! Exercise the complete handshake and route decision on local ephemeral ports.

use smgw_cls::transport::{SmgwProxy, TlsEndpoint};
use std::{fs, io, path::Path, time::Duration};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

const SETUP: Duration = Duration::from_secs(5);
const ROUTE: &str = "meter.test";
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    xtask::generate(dir.path().join("pki").as_path())?;
    Ok(dir)
}

async fn listener() -> io::Result<TcpListener> {
    TcpListener::bind("127.0.0.1:0").await
}

fn proxy(pki: &Path, wan: &TcpListener, setup: Duration, total: Duration) -> Result<SmgwProxy> {
    Ok(SmgwProxy::new(
        TlsEndpoint::load(pki, "han", "smgw", "cls")?,
        TlsEndpoint::load(pki, "wan", "smgw", "aemt")?,
        ROUTE.into(),
        wan.local_addr()?,
        "aemt.test".into(),
        setup,
        total,
    )?)
}

async fn assert_no_wan_dial(listener: &TcpListener) {
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn unknown_sni_opens_no_wan_connection() -> Result<()> {
    let dir = fixture()?;
    let pki = dir.path().join("pki");
    let (han, wan) = (listener().await?, listener().await?);
    let mut gateway = proxy(&pki, &wan, SETUP, SETUP)?;
    let cls = TlsEndpoint::load(&pki, "han", "cls", "smgw")?;
    let address = han.local_addr()?;
    let (client, result) = tokio::join!(cls.connect(address, "unmapped.test", SETUP), async {
        gateway.relay(han.accept().await?.0).await
    });
    let _session = client?;
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_no_wan_dial(&wan).await;
    Ok(())
}

#[tokio::test]
async fn valid_chain_with_unprovisioned_leaf_opens_no_wan_connection() -> Result<()> {
    let dir = fixture()?;
    let pki = dir.path().join("pki");
    let cls = TlsEndpoint::load(&pki, "han", "cls", "smgw")?;
    // The gateway's expected peer differs from the valid client that will connect.
    fs::copy(
        pki.join("han/smgw.chain.pem"),
        pki.join("han/cls.chain.pem"),
    )?;
    let (han, wan) = (listener().await?, listener().await?);
    let mut gateway = proxy(&pki, &wan, SETUP, SETUP)?;
    let address = han.local_addr()?;
    let (client, result) = tokio::join!(cls.connect(address, ROUTE, SETUP), async {
        gateway.relay(han.accept().await?.0).await
    });
    assert!(client.is_err());
    assert!(result.is_err());
    assert_no_wan_dial(&wan).await;
    Ok(())
}

#[tokio::test]
async fn wrong_trust_anchor_rejects_the_pinned_peer() -> Result<()> {
    let dir = fixture()?;
    let pki = dir.path().join("pki");
    let server = TlsEndpoint::load(&pki, "wan", "aemt", "smgw")?;
    fs::copy(pki.join("han/root.pem"), pki.join("wan/root.pem"))?;
    let caller = TlsEndpoint::load(&pki, "wan", "smgw", "aemt")?;
    let wan = listener().await?;
    let (client, result) = tokio::join!(
        caller.connect(wan.local_addr()?, "aemt.test", SETUP),
        async { server.accept(wan.accept().await?.0, SETUP).await }
    );
    assert!(client.is_err());
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn silent_client_expires_before_wan_dial() -> Result<()> {
    let dir = fixture()?;
    let pki = dir.path().join("pki");
    let (han, wan) = (listener().await?, listener().await?);
    let mut gateway = proxy(&pki, &wan, Duration::from_millis(30), SETUP)?;
    let _silent = TcpStream::connect(han.local_addr()?).await?;
    let result = gateway.relay(han.accept().await?.0).await;
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_no_wan_dial(&wan).await;
    Ok(())
}

#[tokio::test]
async fn relay_preserves_payload_larger_than_copy_buffers_and_finishes_on_close() -> Result<()> {
    let dir = fixture()?;
    let pki = dir.path().join("pki");
    let (han, wan) = (listener().await?, listener().await?);
    let mut gateway = proxy(&pki, &wan, SETUP, SETUP)?;
    let cls = TlsEndpoint::load(&pki, "han", "cls", "smgw")?;
    let aemt = TlsEndpoint::load(&pki, "wan", "aemt", "smgw")?;
    let payload: Vec<_> = (0..128 * 1024).map(|n| (n % 251) as u8).collect();
    let (client, relayed, receiver) = timeout(SETUP * 2, async {
        tokio::join!(
            async {
                let mut session = cls.connect(han.local_addr()?, ROUTE, SETUP).await?;
                assert_eq!(session.ssl().version_str(), "TLSv1.2");
                let cipher = session
                    .ssl()
                    .current_cipher()
                    .ok_or_else(|| io::Error::other("no cipher"))?;
                assert_eq!(cipher.name(), "ECDHE-ECDSA-AES128-GCM-SHA256");
                let ephemeral = session.ssl().peer_tmp_key()?.ec_key()?;
                assert_eq!(
                    ephemeral.group().curve_name(),
                    Some(openssl::nid::Nid::BRAINPOOL_P256R1)
                );
                session.write_all(&payload).await?;
                let mut reply = [0; 3];
                session.read_exact(&mut reply).await?;
                assert_eq!(&reply, b"ack");
                session.shutdown().await
            },
            async { gateway.relay(han.accept().await?.0).await },
            async {
                let mut session = aemt.accept(wan.accept().await?.0, SETUP).await?;
                let mut received = vec![0; payload.len()];
                session.read_exact(&mut received).await?;
                assert_eq!(received, payload);
                session.write_all(b"ack").await?;
                session.shutdown().await
            }
        )
    })
    .await?;
    client?;
    receiver?;
    assert_eq!(relayed?, (payload.len() as u64, 3));
    Ok(())
}

#[tokio::test]
async fn missing_client_certificate_and_disjoint_groups_fail_closed() -> Result<()> {
    use openssl::ssl::{SslConnector, SslFiletype, SslMethod, SslVersion};

    let dir = fixture()?;
    let pki = dir.path().join("pki");
    let (han, wan) = (listener().await?, listener().await?);
    let mut gateway = proxy(&pki, &wan, SETUP, SETUP)?;
    for (group, send_certificate) in [("brainpoolP256r1", false), ("P-256", true)] {
        let mut client = SslConnector::builder(SslMethod::tls_client())?;
        client.set_min_proto_version(Some(SslVersion::TLS1_2))?;
        client.set_max_proto_version(Some(SslVersion::TLS1_2))?;
        client.set_groups_list(group)?;
        client.set_ca_file(pki.join("han/root.pem"))?;
        if send_certificate {
            client.set_certificate_chain_file(pki.join("han/cls.chain.pem"))?;
            client.set_private_key_file(pki.join("han/cls.key.pem"), SslFiletype::PEM)?;
        }
        let addr = han.local_addr()?;
        let handshake = tokio::task::spawn_blocking(move || -> Result<bool> {
            let tcp = std::net::TcpStream::connect_timeout(&addr, SETUP)?;
            tcp.set_read_timeout(Some(SETUP))?;
            tcp.set_write_timeout(Some(SETUP))?;
            Ok(client.build().connect("smgw.test", tcp).is_err())
        });
        let rejected = gateway.relay(han.accept().await?.0).await;
        assert!(handshake.await??);
        assert!(rejected.is_err());
        assert_no_wan_dial(&wan).await;
    }
    Ok(())
}

#[tokio::test]
async fn stalled_authenticated_session_expires_and_drops_both_legs() -> Result<()> {
    let dir = fixture()?;
    let pki = dir.path().join("pki");
    let (han, wan) = (listener().await?, listener().await?);
    let mut gateway = proxy(&pki, &wan, SETUP, Duration::from_millis(600))?;
    let cls = TlsEndpoint::load(&pki, "han", "cls", "smgw")?;
    let aemt = TlsEndpoint::load(&pki, "wan", "aemt", "smgw")?;
    let clients = async {
        let mut local = cls.connect(han.local_addr()?, ROUTE, SETUP).await?;
        let mut remote = aemt.accept(wan.accept().await?.0, SETUP).await?;
        let mut byte = [0; 1];
        for side in [&mut local, &mut remote] {
            // A timed-out TLS relay can terminate with unexpected EOF or a socket error.
            assert!(!matches!(side.read(&mut byte).await, Ok(n) if n > 0));
        }
        io::Result::Ok(())
    };
    let ((), result) = timeout(SETUP, async {
        let (peers, relay) = tokio::join!(clients, async {
            gateway.relay(han.accept().await?.0).await
        });
        Ok::<_, io::Error>((peers?, relay))
    })
    .await??;
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    Ok(())
}

#[tokio::test]
async fn cancelling_relay_closes_both_legs_and_releases_slot() -> Result<()> {
    let dir = fixture()?;
    let pki = dir.path().join("pki");
    let (han, wan) = (listener().await?, listener().await?);
    let mut gateway = proxy(&pki, &wan, SETUP, SETUP)?;
    let cls = TlsEndpoint::load(&pki, "han", "cls", "smgw")?;
    let aemt = TlsEndpoint::load(&pki, "wan", "aemt", "smgw")?;
    let endpoints = async {
        let local = cls.connect(han.local_addr()?, ROUTE, SETUP).await?;
        let remote = aemt.accept(wan.accept().await?.0, SETUP).await?;
        io::Result::Ok((local, remote))
    };
    let (mut local, mut remote) = timeout(SETUP, async {
        tokio::select! {
            peers = endpoints => peers,
            outcome = async { gateway.relay(han.accept().await?.0).await } => {
                Err(io::Error::other(format!("relay stopped early: {outcome:?}")))
            }
        }
    })
    .await??;
    for side in [&mut local, &mut remote] {
        assert!(!matches!(timeout(SETUP, side.read(&mut [0; 1])).await?, Ok(n) if n > 0));
    }
    let address = han.local_addr()?;
    let (client, routed) = tokio::join!(cls.connect(address, "unknown.test", SETUP), async {
        gateway.relay(han.accept().await?.0).await
    });
    client?;
    assert_eq!(routed.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    Ok(())
}
