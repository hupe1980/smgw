use openssl::nid::Nid;
use openssl::pkey::PKey;
use openssl::stack::Stack;
use openssl::x509::store::X509StoreBuilder;
use openssl::x509::verify::{X509VerifyFlags, X509VerifyParam};
use openssl::x509::{X509, X509PurposeId, X509StoreContext};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command.arg("gen-pki");
    command
}

fn verify(
    root: &Path,
    certs: &[X509],
    host: &str,
    purpose: X509PurposeId,
    epoch: i64,
) -> xtask::Result<bool> {
    let mut store = X509StoreBuilder::new()?;
    store.add_cert(X509::from_pem(&fs::read(root)?)?)?;
    let mut params = X509VerifyParam::new()?;
    params.set_flags(X509VerifyFlags::X509_STRICT)?;
    params.set_host(host)?;
    params.set_time(epoch as _);
    params.set_purpose(purpose)?;
    store.set_param(&params)?;
    let mut chain = Stack::new()?;
    chain.push(certs[1].clone())?;
    Ok(
        X509StoreContext::new()?
            .init(&store.build(), &certs[0], &chain, |ctx| ctx.verify_cert())?,
    )
}

#[test]
fn generated_roles_verify_only_in_their_own_domain_and_window() -> xtask::Result<()> {
    let tmp = tempdir()?;
    let out = tmp.path().join("target/test-pki");
    assert!(cli().current_dir(tmp.path()).output()?.status.success());
    let manifest = fs::read_to_string(out.join("manifest.tsv"))?;
    assert_eq!(manifest.lines().count(), 5);
    let mut fingerprints = std::collections::HashSet::new();
    for row in manifest.lines().skip(1) {
        let fields: Vec<_> = row.split('\t').collect();
        let [leg, role, tls_role, host, fingerprint, anchor, epoch] = fields[..] else {
            panic!("unexpected fixture manifest row");
        };
        let epoch = epoch.parse()?;
        let dir = out.join(leg);
        let keyfile = dir.join(format!("{role}.key.pem"));
        let key = PKey::private_key_from_pem(&fs::read(&keyfile)?)?;
        let certs = X509::stack_from_pem(&fs::read(dir.join(format!("{role}.chain.pem")))?)?;
        assert_eq!(certs.len(), 2);
        assert!(certs[0].public_key()?.public_eq(&key));
        assert_eq!(
            key.ec_key()?.group().curve_name(),
            Some(Nid::BRAINPOOL_P256R1)
        );
        assert!(fingerprints.insert(fingerprint));
        let expected_fingerprint: String = certs[0]
            .digest(openssl::hash::MessageDigest::sha256())?
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(expected_fingerprint, fingerprint);
        let (purpose, other) = match tls_role {
            "client" => (X509PurposeId::SSL_CLIENT, X509PurposeId::SSL_SERVER),
            "server" => (X509PurposeId::SSL_SERVER, X509PurposeId::SSL_CLIENT),
            role => panic!("unexpected TLS role {role}"),
        };
        let root = out.join(anchor);
        assert!(verify(&root, &certs, host, purpose, epoch)?);
        assert!(!verify(&root, &certs, host, other, epoch)?);
        assert!(!verify(&root, &certs, "unknown.test", purpose, epoch)?);
        assert!(!verify(&root, &certs, host, purpose, epoch + 8 * 86400)?);
        let other_leg = if leg == "han" { "wan" } else { "han" };
        assert!(!verify(
            &out.join(other_leg).join("root.pem"),
            &certs,
            host,
            purpose,
            epoch
        )?);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&out)?.permissions().mode() & 0o077, 0);
            assert_eq!(fs::metadata(keyfile)?.permissions().mode() & 0o077, 0);
        }
    }
    Ok(())
}

#[test]
fn concurrent_generation_publishes_one_complete_set() -> xtask::Result<()> {
    let tmp = tempdir()?;
    let out = tmp.path().join("fixtures");
    let mut children = [cli().arg(&out).spawn()?, cli().arg(&out).spawn()?];
    let statuses = [children[0].wait()?, children[1].wait()?];
    assert_eq!(statuses.iter().filter(|s| s.success()).count(), 1);
    let key = out.join("han/cls.key.pem");
    let original = fs::read(&key)?;
    assert!(!cli().arg(&out).output()?.status.success());
    assert_eq!(original, fs::read(key)?);
    assert_eq!(fs::read_dir(tmp.path())?.count(), 1);
    Ok(())
}
