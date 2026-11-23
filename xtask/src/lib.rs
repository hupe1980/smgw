#![forbid(unsafe_code)]

//! Private test credentials for the CLS loopback simulator.

use openssl::asn1::Asn1Time;
use openssl::bn::{BigNum, MsbOption};
use openssl::ec::{EcGroup, EcKey};
use openssl::error::ErrorStack;
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::x509::extension::{
    AuthorityKeyIdentifier, BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName,
    SubjectKeyIdentifier,
};
use openssl::x509::{X509, X509NameBuilder};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy)]
enum Purpose {
    Authority(u32),
    Client,
    Server,
}

struct Identity {
    cert: X509,
    key: PKey<Private>,
}

fn issue(name: &str, purpose: Purpose, issuer: Option<&Identity>, epoch: i64) -> Result<Identity> {
    let group = EcGroup::from_curve_name(Nid::BRAINPOOL_P256R1)?;
    let key = PKey::from_ec_key(EcKey::generate(&group)?)?;
    let mut subject = X509NameBuilder::new()?;
    subject.append_entry_by_nid(Nid::ORGANIZATIONNAME, "smgw-cls local test fixtures")?;
    subject.append_entry_by_nid(Nid::COMMONNAME, name)?;
    let subject = subject.build();
    let mut cert = X509::builder()?;
    cert.set_version(2)?;
    let mut serial = BigNum::new()?;
    serial.rand(159, MsbOption::ONE, false)?;
    let serial = serial.to_asn1_integer()?;
    cert.set_serial_number(&serial)?;
    cert.set_subject_name(&subject)?;
    cert.set_issuer_name(issuer.map_or(&subject, |ca| ca.cert.subject_name()))?;
    cert.set_pubkey(&key)?;
    let before = Asn1Time::from_unix(epoch - 60)?;
    let after = Asn1Time::from_unix(epoch + 7 * 24 * 60 * 60)?;
    cert.set_not_before(&before)?;
    cert.set_not_after(&after)?;
    let parent = issuer.map(|ca| ca.cert.as_ref());
    let skid = SubjectKeyIdentifier::new().build(&cert.x509v3_context(parent, None))?;
    cert.append_extension(skid)?;
    if parent.is_some() {
        let akid = AuthorityKeyIdentifier::new()
            .keyid(true)
            .build(&cert.x509v3_context(parent, None))?;
        cert.append_extension(akid)?;
    }
    match purpose {
        Purpose::Authority(depth) => {
            cert.append_extension(
                BasicConstraints::new()
                    .critical()
                    .ca()
                    .pathlen(depth)
                    .build()?,
            )?;
            cert.append_extension(
                KeyUsage::new()
                    .critical()
                    .key_cert_sign()
                    .crl_sign()
                    .build()?,
            )?;
        }
        Purpose::Client | Purpose::Server => {
            cert.append_extension(BasicConstraints::new().critical().build()?)?;
            cert.append_extension(KeyUsage::new().critical().digital_signature().build()?)?;
            let mut usage = ExtendedKeyUsage::new();
            match purpose {
                Purpose::Client => usage.client_auth(),
                _ => usage.server_auth(),
            };
            cert.append_extension(usage.build()?)?;
            let san = SubjectAlternativeName::new()
                .dns(name)
                .build(&cert.x509v3_context(parent, None))?;
            cert.append_extension(san)?;
        }
    }
    cert.sign(issuer.map_or(&key, |ca| &ca.key), MessageDigest::sha256())?;
    Ok(Identity {
        cert: cert.build(),
        key,
    })
}

fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents)
}

fn fingerprint(cert: &X509) -> std::result::Result<String, ErrorStack> {
    Ok(cert
        .digest(MessageDigest::sha256())?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn issue_leg(
    base: &Path,
    leg: &str,
    client: &str,
    server: &str,
    epoch: i64,
    manifest: &mut String,
) -> Result<()> {
    let dir = base.join(leg);
    fs::create_dir(&dir)?;
    let root = issue(
        &format!("{leg} test root"),
        Purpose::Authority(1),
        None,
        epoch,
    )?;
    let ca = issue(
        &format!("{leg} test issuer"),
        Purpose::Authority(0),
        Some(&root),
        epoch,
    )?;
    let intermediate = ca.cert.to_pem()?;
    fs::write(dir.join("root.pem"), root.cert.to_pem()?)?;
    fs::write(dir.join("intermediate.pem"), &intermediate)?;
    for (role, purpose, tls_role) in [
        (client, Purpose::Client, "client"),
        (server, Purpose::Server, "server"),
    ] {
        let host = format!("{role}.test");
        let leaf = issue(&host, purpose, Some(&ca), epoch)?;
        let mut chain = leaf.cert.to_pem()?;
        chain.extend_from_slice(&intermediate);
        fs::write(dir.join(format!("{role}.chain.pem")), chain)?;
        write_private(
            &dir.join(format!("{role}.key.pem")),
            &leaf.key.private_key_to_pem_pkcs8()?,
        )?;
        use std::fmt::Write;
        writeln!(
            manifest,
            "{leg}\t{role}\t{tls_role}\t{host}\t{}\t{leg}/root.pem\t{epoch}",
            fingerprint(&leaf.cert)?
        )?;
    }
    Ok(())
}

/// Create a new fixture directory. Private CA keys stay in memory.
pub fn generate(out: &Path) -> Result<()> {
    if out.symlink_metadata().is_ok() {
        return Err(
            io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists").into(),
        );
    }
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut builder = tempfile::Builder::new();
    builder.prefix(".smgw-pki-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let stage = builder.tempdir_in(parent)?;
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .try_into()?;
    let mut manifest =
        String::from("leg\trole\ttls_role\tdns_name\tsha256\ttrust_anchor\tverify_at_unix\n");
    issue_leg(stage.path(), "han", "cls", "smgw", epoch, &mut manifest)?;
    issue_leg(stage.path(), "wan", "smgw", "aemt", epoch, &mut manifest)?;
    fs::write(stage.path().join("manifest.tsv"), manifest)?;
    publish_directory(stage.path(), out)?;
    // The path moved. Prevent cleanup from touching a newly recreated staging path.
    let _ = stage.keep();
    Ok(())
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn publish_directory(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE)?;
    Ok(())
}

#[cfg(windows)]
fn publish_directory(from: &Path, to: &Path) -> io::Result<()> {
    // Windows refuses to replace any existing directory in a directory move.
    fs::rename(from, to)
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    windows
)))]
fn publish_directory(from: &Path, to: &Path) -> io::Result<()> {
    // Reserve this name exclusively before replacing our own empty directory.
    fs::create_dir(to)?;
    fs::rename(from, to).inspect_err(|_| {
        let _ = fs::remove_dir(to);
    })
}
