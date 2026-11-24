//! Offline verification for a single management recipient and a prevalidated
//! GWA signing key. The wire layout is defined in [BSI Detailspecification §2.1].
//! The simulator's freshness policy is strictly less than 30 seconds of skew.
//! [TR-03109-1 v1.1 §3.2.4.3] describes a ±30-second window, while its
//! [test specification, table 4.52] rejects packets at the two endpoints.
//!
//! [BSI Detailspecification §2.1]: https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR-03109-1_Detailspezifikation_v2_0.pdf?__blob=publicationFile&v=2
//! [TR-03109-1 v1.1 §3.2.4.3]: https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR03109-1.pdf?__blob=publicationFile&v=4
//! [test specification, table 4.52]: https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR-03109-1_Testspezifikation.pdf?__blob=publicationFile&v=3

use super::{ParseError, SignatureWidth, parse};
use core::{fmt, num::NonZeroU8, time::Duration};
use openssl::bn::BigNum;
use openssl::ecdsa::EcdsaSig;
use openssl::error::ErrorStack;
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Public};
use openssl::sign::Verifier;
use std::collections::VecDeque;

#[cfg(feature = "wakeup-store")]
mod store;

// 0.4.0.127.0.7.1.1.4.1.3 (ecdsa-plain-SHA256).
const SHA256_OID: &[u8] = &[4, 0, 127, 0, 7, 1, 1, 4, 1, 3];

/// Local rejection reason. A network service should keep these diagnostics local.
#[derive(Debug)]
#[non_exhaustive]
pub enum VerifyError {
    /// Wire-format check failed.
    Parse(ParseError),
    /// Public key is outside the configured Brainpool P-256 profile.
    Key,
    /// The configured gateway identifier is structurally invalid.
    InvalidRecipientId,
    /// The packet addresses a different recipient.
    Recipient,
    /// The algorithm identifier is outside the verifier profile.
    Algorithm,
    /// Absolute skew is 30 or more seconds.
    Freshness,
    /// Signed time is at or below the most recently accepted timestamp.
    Replay,
    /// The caller's monotonic clock regressed.
    ClockRegression,
    /// All allowed signature attempts in the current rolling minute have been used.
    RateLimited,
    /// ECDSA verification failed.
    Signature,
    /// A verifier with an attached database cannot switch its history.
    #[cfg(feature = "wakeup-store")]
    StoreAlreadyConfigured,
    /// The durable replay store could not be opened, read or committed.
    #[cfg(feature = "wakeup-store")]
    Store(redb::Error),
    /// The cryptographic backend returned an error.
    Crypto(ErrorStack),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Parse(e) => return e.fmt(f),
            Self::Key => "expected a valid Brainpool P-256 public key",
            Self::InvalidRecipientId => "invalid configured gateway identifier",
            Self::Recipient => "unexpected management recipient",
            Self::Algorithm => "unsupported management signature algorithm",
            Self::Freshness => "management timestamp outside the freshness window",
            Self::Replay => "management timestamp already accepted or superseded",
            Self::ClockRegression => "monotonic time moved backwards",
            Self::RateLimited => "management signature verification budget exhausted",
            Self::Signature => "invalid management signature",
            #[cfg(feature = "wakeup-store")]
            Self::StoreAlreadyConfigured => "a replay database is already configured",
            #[cfg(feature = "wakeup-store")]
            Self::Store(e) => return write!(f, "wake-up replay database: {e}"),
            Self::Crypto(e) => return e.fmt(f),
        })
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(e) => Some(e),
            #[cfg(feature = "wakeup-store")]
            Self::Store(e) => Some(e),
            Self::Crypto(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ErrorStack> for VerifyError {
    fn from(error: ErrorStack) -> Self {
        Self::Crypto(error)
    }
}

/// Stateful verifier bound to one configured recipient and one GWA signer.
///
/// Retain one instance for its entire verification session. Replay state lives in
/// memory by default. The optional `wakeup-store` feature can persist it across
/// restarts and signing-key rotation. Rate limits are per instance. The timestamp rule
/// also rejects authentic messages arriving out of order or within the same second.
pub struct WakeUpVerifier {
    recipient: [u8; 9],
    key: PKey<Public>,
    budget: NonZeroU8,
    attempts: VecDeque<Duration>,
    last_monotonic: Duration,
    last_accepted: Option<i64>,
    #[cfg(feature = "wakeup-store")]
    store: Option<store::ReplayStore>,
}

impl WakeUpVerifier {
    /// Bind a recipient to the public key from its trusted, currently valid
    /// GWA_WAN_SIG_CRT. The caller owns certificate policy, path and revocation checks.
    pub fn new(
        recipient: [u8; 9],
        key: PKey<Public>,
        attempts_per_minute: NonZeroU8,
    ) -> Result<Self, VerifyError> {
        if !super::valid_recipient(&recipient) {
            return Err(VerifyError::InvalidRecipientId);
        }
        let ec = key.ec_key().map_err(|_| VerifyError::Key)?;
        if ec.group().curve_name() != Some(Nid::BRAINPOOL_P256R1) {
            return Err(VerifyError::Key);
        }
        ec.check_key().map_err(|_| VerifyError::Key)?;
        Ok(Self {
            recipient,
            key,
            budget: attempts_per_minute,
            attempts: VecDeque::with_capacity(usize::from(attempts_per_minute.get())),
            last_monotonic: Duration::ZERO,
            last_accepted: None,
            #[cfg(feature = "wakeup-store")]
            store: None,
        })
    }

    /// Share a durable replay watermark for this recipient across verifiers.
    ///
    /// Opens or creates a redb file in an existing, trusted directory. Keep
    /// using the same file when the GWA signing key changes. Protect the file and
    /// directory against replacement and backup rollback. An accepted timestamp
    /// already held by this instance is carried into the database. A storage
    /// error rejects the operation. Rate limiting remains local to this instance.
    ///
    /// The store uses redb multi-writer mode and immediate durability. Each claim
    /// waits up to 500 ms for writer contention. This budget excludes filesystem
    /// I/O, initial database opening and integrity verification.
    #[cfg(feature = "wakeup-store")]
    pub fn with_replay_database(
        mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<Self, VerifyError> {
        if self.store.is_some() {
            return Err(VerifyError::StoreAlreadyConfigured);
        }
        let store = store::ReplayStore::open(path.as_ref()).map_err(VerifyError::Store)?;
        if let Some(timestamp) = self.last_accepted {
            store
                .advance(&self.recipient, timestamp)
                .map_err(VerifyError::Store)?;
        }
        self.last_accepted = store
            .watermark(&self.recipient)
            .map_err(VerifyError::Store)?;
        self.store = Some(store);
        Ok(self)
    }

    /// Return the signed timestamp on success. Supply trusted synchronized UTC
    /// seconds and a monotonic time with a stable origin. No connection is opened.
    pub fn verify(
        &mut self,
        packet: &[u8],
        synchronized_unix_seconds: i64,
        monotonic: Duration,
    ) -> Result<i64, VerifyError> {
        if monotonic < self.last_monotonic {
            return Err(VerifyError::ClockRegression);
        }
        self.last_monotonic = monotonic;
        let message = parse(packet, SignatureWidth::Bits256).map_err(VerifyError::Parse)?;
        if message.recipient_id != &self.recipient {
            return Err(VerifyError::Recipient);
        }
        if message.algorithm_oid != SHA256_OID {
            return Err(VerifyError::Algorithm);
        }
        if self
            .last_accepted
            .is_some_and(|t| message.unix_seconds <= t)
        {
            return Err(VerifyError::Replay);
        }
        if message.unix_seconds.abs_diff(synchronized_unix_seconds) >= 30 {
            return Err(VerifyError::Freshness);
        }
        while self
            .attempts
            .front()
            .is_some_and(|t| monotonic - *t >= Duration::from_secs(60))
        {
            self.attempts.pop_front();
        }
        if self.attempts.len() == usize::from(self.budget.get()) {
            return Err(VerifyError::RateLimited);
        }
        self.attempts.push_back(monotonic);
        // OpenSSL's EVP verifier takes a DER sequence. The packet carries two
        // fixed-width, big-endian unsigned integers.
        let signature = EcdsaSig::from_private_components(
            BigNum::from_slice(message.r)?,
            BigNum::from_slice(message.s)?,
        )?;
        let mut verifier = Verifier::new(MessageDigest::sha256(), &self.key)?;
        if !verifier.verify_oneshot(&signature.to_der()?, message.signed_bytes)? {
            return Err(VerifyError::Signature);
        }
        #[cfg(feature = "wakeup-store")]
        if let Some(store) = &self.store
            && !store
                .advance(&self.recipient, message.unix_seconds)
                .map_err(VerifyError::Store)?
        {
            return Err(VerifyError::Replay);
        }
        self.last_accepted = Some(message.unix_seconds);
        Ok(message.unix_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::ec::{EcGroup, EcKey};
    use openssl::pkey::Private;
    use openssl::sign::Signer;

    const RECIPIENT: [u8; 9] = *b"\x0eTST\x00\x00\x00\x00\x01";
    const NOW: i64 = 1_700_000_000;
    const ZERO: Duration = Duration::ZERO;

    fn key(curve: Nid) -> PKey<Private> {
        let group = EcGroup::from_curve_name(curve).unwrap();
        PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap()
    }

    fn public(key: &PKey<Private>) -> PKey<Public> {
        PKey::public_key_from_der(&key.public_key_to_der().unwrap()).unwrap()
    }

    fn verifier(key: &PKey<Private>, budget: u8) -> WakeUpVerifier {
        WakeUpVerifier::new(RECIPIENT, public(key), NonZeroU8::new(budget).unwrap()).unwrap()
    }

    fn signed_fixture(key: &PKey<Private>, timestamp: i64) -> [u8; 108] {
        signed_for_recipient(key, timestamp, RECIPIENT)
    }

    fn signed_for_recipient(key: &PKey<Private>, timestamp: i64, recipient: [u8; 9]) -> [u8; 108] {
        let mut packet = [0; 108];
        packet[..3].copy_from_slice(b"WU\x01");
        packet[3..12].copy_from_slice(&recipient);
        packet[12..20].copy_from_slice(&timestamp.to_le_bytes());
        packet[20..32].fill(11);
        packet[32..34].copy_from_slice(&[1, 10]);
        packet[34..44].copy_from_slice(SHA256_OID);
        let mut signer = Signer::new(MessageDigest::sha256(), key).unwrap();
        let signature =
            EcdsaSig::from_der(&signer.sign_oneshot_to_vec(&packet[..32]).unwrap()).unwrap();
        packet[44..76].copy_from_slice(&signature.r().to_vec_padded(32).unwrap());
        packet[76..].copy_from_slice(&signature.s().to_vec_padded(32).unwrap());
        packet
    }

    #[test]
    fn constructor_rejects_invalid_configured_recipient() {
        let public_key = public(&key(Nid::BRAINPOOL_P256R1));
        let mut upper_bound = RECIPIENT;
        upper_bound[4] = 0xfe;
        upper_bound[5..].copy_from_slice(&99_999_999_u32.to_be_bytes());
        assert!(WakeUpVerifier::new(upper_bound, public_key.clone(), NonZeroU8::MIN).is_ok());
        let mut out_of_range = upper_bound;
        out_of_range[5..].copy_from_slice(&100_000_000_u32.to_be_bytes());
        for invalid in [
            [0; 9],
            *b"\x0eTsT\x00\x00\x00\x00\x01",
            *b"\x0eTST\xff\x00\x00\x00\x01",
            out_of_range,
        ] {
            assert!(matches!(
                WakeUpVerifier::new(invalid, public_key.clone(), NonZeroU8::MIN),
                Err(VerifyError::InvalidRecipientId)
            ));
        }
    }

    #[test]
    fn validates_brainpool_signature_and_binds_signed_content() {
        let signer = key(Nid::BRAINPOOL_P256R1);
        let mut validator = verifier(&signer, 4);
        let fixture = signed_fixture(&signer, NOW);
        let mut altered = fixture;
        altered[12] ^= 1;
        assert!(matches!(
            validator.verify(&altered, NOW, ZERO),
            Err(VerifyError::Signature)
        ));
        let other_signer = signed_fixture(&key(Nid::BRAINPOOL_P256R1), NOW);
        assert!(matches!(
            validator.verify(&other_signer, NOW, ZERO),
            Err(VerifyError::Signature)
        ));
        assert_eq!(validator.verify(&fixture, NOW, ZERO).unwrap(), NOW);
        assert!(matches!(
            WakeUpVerifier::new(
                RECIPIENT,
                public(&key(Nid::X9_62_PRIME256V1)),
                NonZeroU8::MIN
            ),
            Err(VerifyError::Key)
        ));
    }

    #[test]
    fn freshness_checks_bounds_and_extreme_timestamps_without_consuming_budget() {
        let signer = key(Nid::BRAINPOOL_P256R1);
        for delta in [-31, -30, -29, 0, 29, 30, 31] {
            let mut validator = verifier(&signer, 1);
            let result = validator.verify(&signed_fixture(&signer, NOW + delta), NOW, ZERO);
            if (-29..=29).contains(&delta) {
                assert_eq!(result.unwrap(), NOW + delta);
            } else {
                assert!(matches!(result, Err(VerifyError::Freshness)));
            }
        }
        let mut validator = verifier(&signer, 1);
        assert!(matches!(
            validator.verify(&signed_fixture(&signer, i64::MIN), i64::MAX, ZERO),
            Err(VerifyError::Freshness)
        ));
        let fixture = signed_fixture(&signer, NOW);
        let mut wrong = fixture;
        wrong[11] ^= 1;
        assert!(matches!(
            validator.verify(&wrong, NOW, ZERO),
            Err(VerifyError::Recipient)
        ));
        wrong = fixture;
        wrong[43] ^= 1;
        assert!(matches!(
            validator.verify(&wrong, NOW, ZERO),
            Err(VerifyError::Algorithm)
        ));
        assert!(validator.verify(&[], NOW, ZERO).is_err());
        assert_eq!(validator.verify(&fixture, NOW, ZERO).unwrap(), NOW);
    }

    #[test]
    fn replay_survives_wall_rollback_and_re_signing() {
        let signer = key(Nid::BRAINPOOL_P256R1);
        let mut validator = verifier(&signer, 2);
        let timestamp = signed_fixture(&signer, NOW);
        validator.verify(&timestamp, NOW, ZERO).unwrap();
        validator
            .verify(&signed_fixture(&signer, NOW + 5), NOW + 5, ZERO)
            .unwrap();
        for replay in [
            timestamp,
            signed_fixture(&signer, NOW),
            signed_fixture(&signer, NOW + 5),
        ] {
            assert!(matches!(
                validator.verify(&replay, NOW, Duration::from_secs(61)),
                Err(VerifyError::Replay)
            ));
        }
    }

    #[test]
    fn rolling_minute_charges_invalid_signatures_and_rejects_monotonic_regression() {
        let signer = key(Nid::BRAINPOOL_P256R1);
        let mut validator = verifier(&signer, 2);
        let valid = signed_fixture(&signer, NOW);
        let invalid = signed_fixture(&key(Nid::BRAINPOOL_P256R1), NOW);
        for t in [0, 30] {
            assert!(matches!(
                validator.verify(&invalid, NOW, Duration::from_secs(t)),
                Err(VerifyError::Signature)
            ));
        }
        assert!(matches!(
            validator.verify(&valid, NOW, Duration::from_millis(59_999)),
            Err(VerifyError::RateLimited)
        ));
        assert!(matches!(
            validator.verify(&valid, NOW, ZERO),
            Err(VerifyError::ClockRegression)
        ));
        assert!(matches!(
            validator.verify(&invalid, NOW, Duration::from_secs(60)),
            Err(VerifyError::Signature)
        ));
        assert!(matches!(
            validator.verify(&valid, NOW, Duration::from_secs(61)),
            Err(VerifyError::RateLimited)
        ));
        assert_eq!(
            validator
                .verify(&valid, NOW, Duration::from_secs(90))
                .unwrap(),
            NOW
        );
    }

    #[cfg(feature = "wakeup-store")]
    mod persistent {
        use super::*;
        use std::sync::{Arc, Barrier};

        #[test]
        fn restart_and_signer_rotation_preserve_only_the_intended_recipient() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("replay.redb");
            let old = key(Nid::BRAINPOOL_P256R1);
            let new = key(Nid::BRAINPOOL_P256R1);
            let mut first = verifier(&old, 4).with_replay_database(&path).unwrap();
            assert_eq!(
                first.verify(&signed_fixture(&old, NOW), NOW, ZERO).unwrap(),
                NOW
            );
            drop(first);
            let mut rotated = verifier(&new, 4).with_replay_database(&path).unwrap();
            for timestamp in [NOW - 1, NOW] {
                assert!(matches!(
                    rotated.verify(&signed_fixture(&new, timestamp), NOW, ZERO),
                    Err(VerifyError::Replay)
                ));
            }
            assert!(matches!(
                rotated.verify(&signed_fixture(&old, NOW + 2), NOW, ZERO),
                Err(VerifyError::Signature)
            ));
            assert_eq!(
                rotated
                    .verify(&signed_fixture(&new, NOW + 1), NOW, ZERO)
                    .unwrap(),
                NOW + 1
            );
            let mut next_recipient = RECIPIENT;
            next_recipient[8] += 1;
            let mut other = WakeUpVerifier::new(next_recipient, public(&new), NonZeroU8::MIN)
                .unwrap()
                .with_replay_database(path)
                .unwrap();
            assert_eq!(
                other
                    .verify(&signed_for_recipient(&new, NOW, next_recipient), NOW, ZERO)
                    .unwrap(),
                NOW
            );
        }

        #[test]
        fn competing_verifiers_commit_an_identical_timestamp_only_once() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("replay.redb");
            let signer = key(Nid::BRAINPOOL_P256R1);
            let packet = signed_fixture(&signer, NOW);
            let ready = Arc::new(Barrier::new(2));
            let worker = || {
                let ready = Arc::clone(&ready);
                let mut v = verifier(&signer, 2).with_replay_database(&path).unwrap();
                std::thread::spawn(move || {
                    ready.wait();
                    match v.verify(&packet, NOW, ZERO) {
                        Ok(_) => true,
                        Err(VerifyError::Replay) => false,
                        Err(other) => panic!("{other}"),
                    }
                })
            };
            let first = worker();
            let second = worker();
            assert_ne!(first.join().unwrap(), second.join().unwrap());
        }

        #[test]
        fn storage_failure_does_not_accept_or_poison_the_watermark() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("replay.redb");
            let signer = key(Nid::BRAINPOOL_P256R1);
            let packet = signed_fixture(&signer, NOW);
            let mut v = verifier(&signer, 4).with_replay_database(&path).unwrap();
            let peer = redb::Database::builder()
                .set_concurrency_mode(redb::ConcurrencyMode::MultiWriter)
                .create(&path)
                .unwrap();
            let lock = peer.begin_write().unwrap();
            assert!(matches!(
                v.verify(&packet, NOW, ZERO),
                Err(VerifyError::Store(redb::Error::WouldBlock))
            ));
            lock.abort().unwrap();
            assert_eq!(v.verify(&packet, NOW, ZERO).unwrap(), NOW);
            assert!(matches!(
                v.verify(&packet, NOW, ZERO),
                Err(VerifyError::Replay)
            ));
        }

        #[test]
        fn attaching_after_acceptance_carries_history_and_rebinding_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("replay.redb");
            let signer = key(Nid::BRAINPOOL_P256R1);
            let mut local = verifier(&signer, 2);
            let packet = signed_fixture(&signer, NOW);
            local.verify(&packet, NOW, ZERO).unwrap();
            let persisted = local.with_replay_database(&path).unwrap();
            let other_path = dir.path().join("different.redb");
            assert!(matches!(
                persisted.with_replay_database(&other_path),
                Err(VerifyError::StoreAlreadyConfigured)
            ));
            assert!(!other_path.exists());
            let mut restarted = verifier(&signer, 2).with_replay_database(path).unwrap();
            assert!(matches!(
                restarted.verify(&packet, NOW, ZERO),
                Err(VerifyError::Replay)
            ));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn non_utf8_paths_retain_history_across_reopen() {
            use std::os::unix::ffi::OsStrExt;

            let directory = tempfile::tempdir().unwrap();
            let path = directory
                .path()
                .join(std::ffi::OsStr::from_bytes(b"history-\xff.redb"));
            let signer = key(Nid::BRAINPOOL_P256R1);
            let packet = signed_fixture(&signer, NOW);
            let mut first = verifier(&signer, 1).with_replay_database(&path).unwrap();
            assert_eq!(first.verify(&packet, NOW, ZERO).unwrap(), NOW);
            drop(first);
            let mut reopened = verifier(&signer, 1).with_replay_database(path).unwrap();
            assert!(matches!(
                reopened.verify(&packet, NOW, ZERO),
                Err(VerifyError::Replay)
            ));
        }

        #[test]
        fn empty_unusable_and_corrupt_stores_fail_closed() {
            let dir = tempfile::tempdir().unwrap();
            let corrupt = dir.path().join("broken.redb");
            std::fs::write(&corrupt, b"invalid replay database").unwrap();
            let signer = key(Nid::BRAINPOOL_P256R1);
            for path in [std::path::Path::new(""), dir.path(), &corrupt] {
                assert!(
                    matches!(
                        verifier(&signer, 1).with_replay_database(path),
                        Err(VerifyError::Store(_))
                    ),
                    "{path:?}"
                );
            }
        }
    }
}
