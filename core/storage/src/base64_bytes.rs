//! Base64 for the bytes in a captured exchange.
//!
//! A body is bytes, and so is the occasional header value; neither is
//! necessarily text, and JSON has no way to carry a byte string. serde's own
//! default for `Vec<u8>` is an array of numbers, which is both large and
//! unreadable, so the model spells raw bytes as a base64 string instead.
//!
//! Used through `#[serde(with = "crate::base64_bytes")]` here, and through
//! `#[serde(with = "proxy_storage::base64_bytes")]` by the other core crates
//! that carry raw bytes in JSON — a replayed request body, for one. It is
//! public for that reason and no other: one encoding of a byte string across
//! the core, rather than a second one written beside it that has to agree.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serializer};

/// Writes bytes as a standard, padded base64 string.
pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&STANDARD.encode(bytes))
}

/// Reads bytes back from a base64 string.
///
/// A string that is not base64 is a deserialization error rather than an
/// empty body: a body that cannot be read back is not the same as one that
/// was never there.
pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let encoded = String::deserialize(deserializer)?;

    STANDARD
        .decode(encoded.as_bytes())
        .map_err(serde::de::Error::custom)
}

/// The same encoding, for a field that may not be there at all.
///
/// An update carries only the fields it changes, so its body is an
/// `Option<Vec<u8>>`: absent means "leave the body as it is" and present
/// means "this is the body now". `#[serde(with)]` needs a module whose
/// functions take exactly that, which is what this one is — the encoding
/// itself is the module above, and there is no second base64 here.
pub mod optional {
    use serde::{Deserialize, Deserializer, Serializer};

    /// Writes the bytes as base64 when there are any.
    ///
    /// Only reached for a field that is being written at all: the fields
    /// that use this are skipped when they are `None`, so the `null` branch
    /// exists to be correct rather than to be taken.
    pub fn serialize<S>(bytes: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match bytes {
            Some(bytes) => super::serialize(bytes, serializer),
            None => serializer.serialize_none(),
        }
    }

    /// Reads the bytes back, or nothing when the field carried nothing.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let Some(encoded) = Option::<String>::deserialize(deserializer)? else {
            return Ok(None);
        };

        base64::Engine::decode(&super::STANDARD, encoded.as_bytes())
            .map(Some)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    /// A field of raw bytes, as every body and binary header value is.
    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Wrapper {
        #[serde(with = "super")]
        bytes: Vec<u8>,
    }

    fn wrapper(bytes: &[u8]) -> Wrapper {
        Wrapper {
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn bytes_become_a_base64_string() {
        let json = serde_json::to_string(&wrapper(b"hello")).expect("serializes");

        assert_eq!(json, r#"{"bytes":"aGVsbG8="}"#);
    }

    #[test]
    fn no_bytes_become_an_empty_string() {
        let json = serde_json::to_string(&wrapper(b"")).expect("serializes");

        assert_eq!(json, r#"{"bytes":""}"#);
    }

    #[test]
    fn every_byte_value_survives_a_round_trip() {
        let all: Vec<u8> = (0..=255).collect();

        let json = serde_json::to_string(&wrapper(&all)).expect("serializes");
        let back: Wrapper = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back.bytes, all);
    }

    #[test]
    fn bytes_that_are_not_utf8_survive_a_round_trip() {
        // A lone continuation byte and an unpaired surrogate's encoding:
        // nothing here is text, and none of it may be lost.
        let bytes = [0x80, 0xed, 0xa0, 0x80, 0xff, 0x00, 0xfe];

        let json = serde_json::to_string(&wrapper(&bytes)).expect("serializes");
        let back: Wrapper = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back.bytes, bytes);
    }

    #[test]
    fn a_string_that_is_not_base64_is_refused() {
        let error = serde_json::from_str::<Wrapper>(r#"{"bytes":"not base64!"}"#)
            .expect_err("invalid base64");

        assert!(error.is_data(), "{error}");
    }
}
