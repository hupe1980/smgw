//! Offline inspection of a gateway administrator (GWA) wake-up message addressed
//! to a smart meter gateway (SMGW).
//!
//! The layout follows [BSI TR-03109-1 Detailspecification v2.0], §2.1, tables
//! 2.1–2.2. The timestamp follows the table's explicit least-significant-byte-first
//! rule. Its displayed hex example uses the opposite order.
//!
//! Parsing returns borrowed, unauthenticated fields. Enable `wakeup-verify`
//! to check the configured recipient, signature, freshness and replay history
//! with a prevalidated GWA signing key.
//!
//! [BSI TR-03109-1 Detailspecification v2.0]: https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR-03109-1_Detailspezifikation_v2_0.pdf?__blob=publicationFile&v=2

#[cfg(feature = "wakeup-verify")]
pub mod verify;

use core::fmt;

/// Width of one plain ECDSA signature component, selected from the trusted
/// signing key's curve. Selecting a width makes no algorithm-policy decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureWidth {
    /// 32 octets per integer.
    Bits256,
    /// 48 octets per integer.
    Bits384,
    /// 64 octets per integer.
    Bits512,
}

impl SignatureWidth {
    fn octets(self) -> usize {
        match self {
            Self::Bits256 => 32,
            Self::Bits384 => 48,
            Self::Bits512 => 64,
        }
    }
}

/// Structurally parsed, unauthenticated fields borrowed from one complete datagram.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnverifiedWakeUp<'a> {
    /// The nine wire octets of the gateway's DIN 43849 equipment identifier.
    pub recipient_id: &'a [u8; 9],
    /// Sender-supplied UTC seconds since the Unix epoch.
    pub unix_seconds: i64,
    /// Exact wire bytes covered by the signature.
    pub signed_bytes: &'a [u8; 32],
    /// Encoded OID contents, without an ASN.1 tag or length.
    pub algorithm_oid: &'a [u8],
    /// Plain big-endian ECDSA integer r.
    pub r: &'a [u8],
    /// Plain big-endian ECDSA integer s.
    pub s: &'a [u8],
}

/// The structural check that failed. All inputs are treated as untrusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Truncated data, trailing data, or signature width inconsistent with the configured key.
    Length,
    /// Incorrect wake-up marker.
    Header,
    /// Unsupported protocol version.
    Version,
    /// Invalid equipment identifier.
    Recipient,
    /// Malformed version-one reserved bytes.
    Padding,
    /// Unsupported signature encoding.
    SignatureFormat,
    /// Empty, oversized or malformed algorithm object identifier.
    AlgorithmOid,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Length => "invalid wake-up message length",
            Self::Header => "invalid wake-up header",
            Self::Version => "unsupported wake-up version",
            Self::Recipient => "invalid wake-up recipient identifier",
            Self::Padding => "invalid wake-up reserved bytes",
            Self::SignatureFormat => "unsupported wake-up signature format",
            Self::AlgorithmOid => "invalid wake-up signature algorithm identifier",
        })
    }
}

impl core::error::Error for ParseError {}

fn valid_recipient(id: &[u8; 9]) -> bool {
    let [0x0e, manufacturer @ .., block, n1, n2, n3, n4] = id else {
        return false;
    };
    manufacturer.iter().all(u8::is_ascii_uppercase)
        && *block != 0xff
        && u32::from_be_bytes([*n1, *n2, *n3, *n4]) <= 99_999_999
}

/// Inspect a complete version-one message with fixed space and bounded work.
///
/// ```
/// use smgw::wakeup::{parse, ParseError, SignatureWidth};
/// assert_eq!(parse(&[], SignatureWidth::Bits256), Err(ParseError::Length));
/// ```
pub fn parse(packet: &[u8], width: SignatureWidth) -> Result<UnverifiedWakeUp<'_>, ParseError> {
    let (signed_bytes, rest) = packet.split_first_chunk::<32>().ok_or(ParseError::Length)?;
    let [b'W', b'U', version, payload @ ..] = signed_bytes else {
        return Err(ParseError::Header);
    };
    if *version != 1 {
        return Err(ParseError::Version);
    }
    let (recipient_id, rest_payload) =
        payload.split_first_chunk::<9>().ok_or(ParseError::Length)?;
    if !valid_recipient(recipient_id) {
        return Err(ParseError::Recipient);
    }
    let (timestamp, padding) = rest_payload
        .split_first_chunk::<8>()
        .ok_or(ParseError::Length)?;
    if padding != [0x0b; 12] {
        return Err(ParseError::Padding);
    }
    let ([format, oid_len], tail) = rest.split_first_chunk::<2>().ok_or(ParseError::Length)?;
    if *format != 1 {
        return Err(ParseError::SignatureFormat);
    }
    if !(1..=14).contains(oid_len) {
        return Err(ParseError::AlgorithmOid);
    }
    let (algorithm_oid, signature) = tail
        .split_at_checked(usize::from(*oid_len))
        .ok_or(ParseError::Length)?;
    if signature.len() != width.octets() * 2 {
        return Err(ParseError::Length);
    }
    // OID subidentifiers use minimal base-128 groups and must terminate.
    let mut start = true;
    for &octet in algorithm_oid {
        if start && octet == 0x80 {
            return Err(ParseError::AlgorithmOid);
        }
        start = octet & 0x80 == 0;
    }
    if !start {
        return Err(ParseError::AlgorithmOid);
    }
    let (r, s) = signature.split_at(width.octets());
    Ok(UnverifiedWakeUp {
        recipient_id,
        unix_seconds: i64::from_le_bytes(*timestamp),
        signed_bytes,
        algorithm_oid,
        r,
        s,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Dummy signature bytes exercise the wire parser only.
    fn fixture() -> [u8; 108] {
        let mut bytes = [0; 108];
        bytes[..12].copy_from_slice(b"WU\x01\x0eTST\x00\x00\x00\x00\x01");
        bytes[12..20].copy_from_slice(&(-3_i64).to_le_bytes());
        bytes[20..32].fill(0x0b);
        bytes[32..44].copy_from_slice(&[1, 10, 4, 0, 127, 0, 7, 1, 1, 4, 1, 3]);
        bytes[76..].fill(0x42);
        bytes
    }

    #[test]
    fn exposes_exact_signed_prefix_and_signature_components() {
        let bytes = fixture();
        let message = parse(&bytes, SignatureWidth::Bits256).unwrap();
        assert_eq!(message.unix_seconds, -3);
        assert_eq!(message.recipient_id, &bytes[3..12]);
        assert_eq!(message.signed_bytes, &bytes[..32]);
        assert_eq!(message.algorithm_oid, &bytes[34..44]);
        assert_eq!(message.r, &[0; 32]);
        assert_eq!(message.s, &[0x42; 32]);
    }

    #[test]
    fn rejects_truncation_trailing_bytes_and_wrong_key_width() {
        let mut bytes = [0; 109];
        bytes[..108].copy_from_slice(&fixture());
        for len in (0..108).chain([109]) {
            assert!(parse(&bytes[..len], SignatureWidth::Bits256).is_err());
        }
        assert_eq!(
            parse(&bytes[..108], SignatureWidth::Bits384),
            Err(ParseError::Length)
        );
        let mut wider = [0; 172];
        wider[..108].copy_from_slice(&fixture());
        assert_eq!(
            parse(&wider[..140], SignatureWidth::Bits384)
                .unwrap()
                .s
                .len(),
            48
        );
        assert_eq!(parse(&wider, SignatureWidth::Bits512).unwrap().s.len(), 64);
    }

    #[test]
    fn rejects_invalid_structure_before_exposing_fields() {
        for (offset, value, expected) in [
            (0, 0, ParseError::Header),
            (2, 2, ParseError::Version),
            (3, 0, ParseError::Recipient),
            (4, b'a', ParseError::Recipient),
            (7, 0xff, ParseError::Recipient),
            (8, 6, ParseError::Recipient),
            (29, 0, ParseError::Padding),
            (32, 0, ParseError::SignatureFormat),
            (33, 0, ParseError::AlgorithmOid),
            (33, 15, ParseError::AlgorithmOid),
            (34, 0x80, ParseError::AlgorithmOid),
            (43, 0x81, ParseError::AlgorithmOid),
        ] {
            let mut bytes = fixture();
            bytes[offset] = value;
            assert_eq!(parse(&bytes, SignatureWidth::Bits256), Err(expected));
        }
    }
}
