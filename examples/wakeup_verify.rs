//! Verify archived management wake-up datagrams against an explicitly trusted GWA key.
use std::{
    env,
    error::Error,
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read},
    num::NonZeroU8,
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
};

use openssl::pkey::PKey;
use smgw_cls::wakeup::verify::WakeUpVerifier;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "Verify management wake-up datagrams offline.

Usage:
  wakeup_verify [--replay-db PATH] --fixture
  wakeup_verify [--replay-db PATH] PUBLIC_KEY.pem RECIPIENT_HEX UNIX_SECONDS PACKET [PACKET ...]

PUBLIC_KEY.pem is a previously authenticated GWA signing key (PEM SubjectPublicKeyInfo).
RECIPIENT_HEX is the expected nine-byte device identifier (18 hexadecimal digits).
UNIX_SECONDS is the trusted UTC reference time for this batch.
Supply up to 16 raw binary datagrams in arrival order.

The default replay history lasts for one invocation. With feature wakeup-store,
--replay-db PATH shares recipient history across invocations and key rotation.
Use a redb file in a trusted local directory. The signature budget is per verifier.";
const MAX_BATCH: NonZeroU8 = NonZeroU8::new(16).unwrap();
const MAX_PUBLIC_KEY: usize = 16 * 1024;
const MAX_PACKET: usize = 32 + 2 + 14 + 2 * 64;
const FIXTURE_PACKET: &[u8] = include_bytes!("fixtures/wakeup-v1.bin");
const FIXTURE_KEY: &[u8] = include_bytes!("fixtures/wakeup-gwa-public.pem");
const FIXTURE_RECIPIENT: [u8; 9] = [0x0e, b'T', b'S', b'T', 0, 0, 0, 0, 1];
const FIXTURE_TIME: i64 = 1_735_689_600;

fn bounded_read(reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("input exceeds {limit} bytes"),
        ));
    }
    Ok(bytes)
}

fn read_file(path: &OsStr, limit: usize) -> Result<Vec<u8>> {
    if !fs::metadata(path)?.is_file() {
        return Err("expected a regular file".into());
    }
    Ok(bounded_read(File::open(path)?, limit)?)
}

fn recipient(text: &OsStr) -> Result<[u8; 9]> {
    let text = text.to_str().ok_or("recipient must be hexadecimal")?;
    if text.len() != 18 || !text.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("recipient must contain exactly 18 hexadecimal digits".into());
    }
    let mut id = [0; 9];
    for (index, octet) in id.iter_mut().enumerate() {
        *octet = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)?;
    }
    Ok(id)
}

fn verifier(key: &[u8], id: [u8; 9], database: Option<&Path>) -> Result<WakeUpVerifier> {
    let key = PKey::public_key_from_pem(key)?;
    let verifier = WakeUpVerifier::new(id, key, MAX_BATCH)?;
    if let Some(path) = database {
        #[cfg(feature = "wakeup-store")]
        return verifier
            .with_replay_database(path)
            .map_err(|error| format!("replay database {}: {error}", path.display()).into());
        #[cfg(not(feature = "wakeup-store"))]
        return Err(format!(
            "--replay-db {} requires --features wakeup-store",
            path.display()
        )
        .into());
    }
    Ok(verifier)
}

fn run() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let mut first = args.next().ok_or(USAGE)?;
    let database = if first == "--replay-db" {
        let path = args.next().ok_or("--replay-db requires a path")?;
        if path.is_empty() {
            return Err("--replay-db requires a path".into());
        }
        first = args.next().ok_or(USAGE)?;
        Some(std::path::PathBuf::from(path))
    } else {
        None
    };
    if first == "-h" || first == "--help" {
        println!("{USAGE}");
        return Ok(());
    }
    if first == "--fixture" {
        if args.next().is_some() {
            return Err(USAGE.into());
        }
        let timestamp = verifier(FIXTURE_KEY, FIXTURE_RECIPIENT, database.as_deref())?.verify(
            FIXTURE_PACKET,
            FIXTURE_TIME,
            Duration::ZERO,
        )?;
        println!("Bundled signature verified at fixture time {timestamp}.");
        return Ok(());
    }
    let id = recipient(&args.next().ok_or(USAGE)?)?;
    let now: i64 = args
        .next()
        .ok_or(USAGE)?
        .to_str()
        .ok_or("invalid Unix time")?
        .parse()?;
    if args.len() == 0 || args.len() > usize::from(MAX_BATCH.get()) {
        return Err(USAGE.into());
    }
    let key = read_file(&first, MAX_PUBLIC_KEY)
        .map_err(|err| format!("key {}: {err}", Path::new(&first).display()))?;
    let mut verifier = verifier(&key, id, database.as_deref())?;
    let start = Instant::now();
    for path in args {
        let name = Path::new(&path).display();
        let bytes = read_file(&path, MAX_PACKET).map_err(|err| format!("{name}: {err}"))?;
        let timestamp = verifier
            .verify(&bytes, now, start.elapsed())
            .map_err(|err| format!("{name}: {err}"))?;
        println!("Verified {name} (signed Unix time {timestamp})");
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wakeup_verify: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_wire_vector_and_process_local_replay() -> Result<()> {
        let mut active = verifier(FIXTURE_KEY, FIXTURE_RECIPIENT, None)?;
        assert_eq!(
            active.verify(FIXTURE_PACKET, FIXTURE_TIME, Duration::ZERO)?,
            FIXTURE_TIME
        );
        assert!(
            active
                .verify(FIXTURE_PACKET, FIXTURE_TIME, Duration::from_secs(1))
                .is_err()
        );
        assert_eq!(
            verifier(FIXTURE_KEY, FIXTURE_RECIPIENT, None)?.verify(
                FIXTURE_PACKET,
                FIXTURE_TIME,
                Duration::ZERO
            )?,
            FIXTURE_TIME
        );
        Ok(())
    }

    #[test]
    fn refuses_damaged_public_vector() -> Result<()> {
        let mut altered = FIXTURE_PACKET.to_vec();
        *altered.last_mut().unwrap() ^= 1;
        assert!(
            verifier(FIXTURE_KEY, FIXTURE_RECIPIENT, None)?
                .verify(&altered, FIXTURE_TIME, Duration::ZERO)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn checks_file_and_recipient_bounds() {
        assert!(bounded_read(&[0; MAX_PACKET + 1][..], MAX_PACKET).is_err());
        assert_eq!(
            bounded_read(&[0; MAX_PACKET][..], MAX_PACKET)
                .unwrap()
                .len(),
            MAX_PACKET
        );
        assert!(recipient("0e545354000000ß01".as_ref()).is_err());
        assert_eq!(
            recipient("0e5453540000000001".as_ref()).unwrap(),
            FIXTURE_RECIPIENT
        );
    }
}
