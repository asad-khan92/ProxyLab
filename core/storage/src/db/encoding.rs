//! Turning model values into column values, and back.
//!
//! Every conversion here is exact in both directions: what goes into the
//! database comes back out as the value it was. That is why the timestamp is
//! two columns and not one, and why a header list is JSON rather than
//! something flattened for display.
//!
//! Nothing in here is a rendering. A stored value is the value, not a
//! formatted version of it, so a UI is free to format it however it likes and
//! a later version of this crate can still read it.

use std::str::FromStr;

use crate::body::BodyCapture;
use crate::error::{TrafficError, TrafficErrorKind};
use crate::headers::Headers;
use crate::timing::Timestamp;
use crate::traffic::{HttpVersion, Scheme};

use super::error::StorageError;

/// Which half of an exchange a stored message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MessageRole {
    /// What the client asked for.
    Request,
    /// What came back.
    Response,
}

impl MessageRole {
    /// The `role` column's value.
    pub(super) fn as_column(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
        }
    }

    /// The role a stored `role` column names.
    pub(super) fn from_column(text: &str) -> Result<Self, StorageError> {
        match text {
            "request" => Ok(Self::Request),
            "response" => Ok(Self::Response),
            _ => Err(StorageError::unreadable("message role")),
        }
    }
}

/// The wall clock split into the millisecond it names and the nanoseconds
/// inside that millisecond.
///
/// The millisecond is what the traffic list sorts and filters on, so it is a
/// column of its own with an index on it; the remainder is what makes the
/// timestamp come back exactly as it went in.
///
/// A clock set far enough into the future to overflow a signed 64-bit count
/// of milliseconds — some 292 million years — saturates rather than wraps.
/// Nothing sensible can be stored for such a clock, and losing the exchange
/// would be worse than storing it at the largest time there is.
pub(super) fn timestamp_columns(timestamp: Timestamp) -> (i64, i64) {
    let millis = timestamp
        .seconds
        .saturating_mul(1_000)
        .saturating_add(u64::from(timestamp.nanos) / 1_000_000);

    (
        i64::try_from(millis).unwrap_or(i64::MAX),
        i64::from(timestamp.nanos % 1_000_000),
    )
}

/// The timestamp those two columns name.
pub(super) fn timestamp_from_columns(millis: i64, subms_nanos: i64) -> Timestamp {
    let millis = millis.max(0) as u64;
    let subms_nanos = subms_nanos.clamp(0, 999_999) as u32;

    Timestamp {
        seconds: millis / 1_000,
        nanos: (millis % 1_000) as u32 * 1_000_000 + subms_nanos,
    }
}

/// A count of microseconds as a column value.
pub(super) fn micros_column(micros: Option<u64>) -> Option<i64> {
    micros.map(|micros| i64::try_from(micros).unwrap_or(i64::MAX))
}

/// The count of microseconds a column holds.
pub(super) fn micros_from_column(column: Option<i64>) -> Option<u64> {
    column.map(|micros| micros.max(0) as u64)
}

/// When the response head arrived, worked out from the start and the offset.
///
/// Stored so a UI can ask when a response came back without doing the
/// arithmetic itself, and left `None` when no response head ever arrived.
pub(super) fn response_at_ms(
    started_at: Timestamp,
    upstream_response_micros: Option<u64>,
) -> Option<i64> {
    let offset = upstream_response_micros?;
    let (started_ms, _) = timestamp_columns(started_at);

    Some(started_ms.saturating_add(i64::try_from(offset / 1_000).unwrap_or(i64::MAX)))
}

/// The `scheme` column's value.
pub(super) fn scheme_column(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Http => "http",
        Scheme::Https => "https",
    }
}

/// The scheme a stored `scheme` column names.
pub(super) fn scheme_from_column(text: &str) -> Result<Scheme, StorageError> {
    match text {
        "http" => Ok(Scheme::Http),
        "https" => Ok(Scheme::Https),
        _ => Err(StorageError::unreadable("scheme")),
    }
}

/// The version column's value, which is the version as HTTP spells it.
pub(super) fn version_column(version: HttpVersion) -> &'static str {
    match version {
        HttpVersion::Http09 => "HTTP/0.9",
        HttpVersion::Http10 => "HTTP/1.0",
        HttpVersion::Http11 => "HTTP/1.1",
        HttpVersion::Http2 => "HTTP/2.0",
        HttpVersion::Http3 => "HTTP/3.0",
        HttpVersion::Unknown => "unknown",
    }
}

/// The version a stored version column names.
///
/// A version this build has no name for reads back as
/// [`HttpVersion::Unknown`], which is the model's own word for exactly that,
/// rather than failing the whole exchange.
pub(super) fn version_from_column(text: &str) -> HttpVersion {
    match text {
        "HTTP/0.9" => HttpVersion::Http09,
        "HTTP/1.0" => HttpVersion::Http10,
        "HTTP/1.1" => HttpVersion::Http11,
        "HTTP/2.0" => HttpVersion::Http2,
        "HTTP/3.0" => HttpVersion::Http3,
        _ => HttpVersion::Unknown,
    }
}

/// The `error_kind` column's value.
pub(super) fn error_kind_column(kind: TrafficErrorKind) -> &'static str {
    match kind {
        TrafficErrorKind::RequestRejected => "request_rejected",
        TrafficErrorKind::ProxyFailure => "proxy_failure",
        TrafficErrorKind::UpstreamConnect => "upstream_connect",
        TrafficErrorKind::UpstreamTls => "upstream_tls",
        TrafficErrorKind::ClientTls => "client_tls",
        TrafficErrorKind::UpstreamHttp => "upstream_http",
        TrafficErrorKind::ResponseForwarding => "response_forwarding",
        TrafficErrorKind::ClientDisconnected => "client_disconnected",
        TrafficErrorKind::Incomplete => "incomplete",
    }
}

/// The kind a stored `error_kind` column names.
pub(super) fn error_kind_from_column(text: &str) -> Result<TrafficErrorKind, StorageError> {
    match text {
        "request_rejected" => Ok(TrafficErrorKind::RequestRejected),
        "proxy_failure" => Ok(TrafficErrorKind::ProxyFailure),
        "upstream_connect" => Ok(TrafficErrorKind::UpstreamConnect),
        "upstream_tls" => Ok(TrafficErrorKind::UpstreamTls),
        "client_tls" => Ok(TrafficErrorKind::ClientTls),
        "upstream_http" => Ok(TrafficErrorKind::UpstreamHttp),
        "response_forwarding" => Ok(TrafficErrorKind::ResponseForwarding),
        "client_disconnected" => Ok(TrafficErrorKind::ClientDisconnected),
        "incomplete" => Ok(TrafficErrorKind::Incomplete),
        _ => Err(StorageError::unreadable("error kind")),
    }
}

/// The error a pair of stored error columns names, if there was one.
pub(super) fn error_from_columns(
    kind: Option<String>,
    summary: Option<String>,
) -> Result<Option<TrafficError>, StorageError> {
    let Some(kind) = kind else {
        return Ok(None);
    };
    let kind = error_kind_from_column(&kind)?;
    let summary = summary.ok_or_else(|| StorageError::unreadable("error summary"))?;

    Ok(Some(TrafficError::restored(kind, summary)))
}

/// A header list as the JSON it is stored as.
///
/// The model's own serialized form, so the order of the headers, a name that
/// appears more than once, and a value that is not text all survive. It is
/// deterministic: the same header list always produces the same string.
pub(super) fn headers_column(headers: &Headers) -> Result<String, StorageError> {
    serde_json::to_string(headers).map_err(|_| StorageError::unreadable("headers"))
}

/// The header list a stored `headers` column holds.
pub(super) fn headers_from_column(json: &str) -> Result<Headers, StorageError> {
    serde_json::from_str(json).map_err(|_| StorageError::unreadable("headers"))
}

/// The captured body those stored columns describe.
///
/// A capture that was cut short says so and says how long the body really
/// was; one that was not is exactly as long as its bytes.
pub(super) fn body_from_columns(data: Vec<u8>, size: i64, truncated: bool) -> BodyCapture {
    let size = size.max(0) as u64;

    if truncated {
        BodyCapture::truncated(data, size)
    } else {
        BodyCapture::complete(data)
    }
}

/// A client address read back from its column.
pub(super) fn client_from_column(text: &str) -> Result<std::net::SocketAddr, StorageError> {
    std::net::SocketAddr::from_str(text).map_err(|_| StorageError::unreadable("client address"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_KIND: &[TrafficErrorKind] = &[
        TrafficErrorKind::RequestRejected,
        TrafficErrorKind::ProxyFailure,
        TrafficErrorKind::UpstreamConnect,
        TrafficErrorKind::UpstreamTls,
        TrafficErrorKind::ClientTls,
        TrafficErrorKind::UpstreamHttp,
        TrafficErrorKind::ResponseForwarding,
        TrafficErrorKind::ClientDisconnected,
        TrafficErrorKind::Incomplete,
    ];

    const EVERY_VERSION: &[HttpVersion] = &[
        HttpVersion::Http09,
        HttpVersion::Http10,
        HttpVersion::Http11,
        HttpVersion::Http2,
        HttpVersion::Http3,
        HttpVersion::Unknown,
    ];

    #[test]
    fn a_timestamp_round_trips_through_its_columns() {
        for timestamp in [
            Timestamp::default(),
            Timestamp {
                seconds: 1_700_000_000,
                nanos: 123_456_789,
            },
            Timestamp {
                seconds: 1,
                nanos: 999_999_999,
            },
            Timestamp::now(),
        ] {
            let (millis, subms) = timestamp_columns(timestamp);

            assert_eq!(timestamp_from_columns(millis, subms), timestamp);
        }
    }

    #[test]
    fn the_millisecond_column_is_the_millisecond_the_timestamp_names() {
        let timestamp = Timestamp {
            seconds: 2,
            nanos: 500_000_500,
        };

        let (millis, subms) = timestamp_columns(timestamp);

        assert_eq!(millis, 2_500);
        assert_eq!(millis as u64, timestamp.as_millis());
        assert_eq!(subms, 500);
    }

    #[test]
    fn an_implausible_clock_saturates_rather_than_wraps() {
        let (millis, _) = timestamp_columns(Timestamp {
            seconds: u64::MAX,
            nanos: 0,
        });

        assert_eq!(millis, i64::MAX);
    }

    #[test]
    fn columns_that_make_no_sense_are_read_as_the_nearest_that_does() {
        assert_eq!(timestamp_from_columns(-5, -5), Timestamp::default());
        assert_eq!(micros_from_column(Some(-5)), Some(0));
    }

    #[test]
    fn a_duration_round_trips_through_its_column() {
        assert_eq!(micros_from_column(micros_column(None)), None);
        assert_eq!(micros_from_column(micros_column(Some(0))), Some(0));
        assert_eq!(micros_from_column(micros_column(Some(3_500))), Some(3_500));
    }

    #[test]
    fn the_response_time_is_the_start_plus_the_offset() {
        let started_at = Timestamp {
            seconds: 10,
            nanos: 0,
        };

        assert_eq!(response_at_ms(started_at, Some(2_500_000)), Some(12_500));
        assert_eq!(response_at_ms(started_at, None), None);
    }

    #[test]
    fn every_scheme_round_trips_through_its_column() {
        for scheme in [Scheme::Http, Scheme::Https] {
            assert_eq!(
                scheme_from_column(scheme_column(scheme)).expect("a scheme"),
                scheme
            );
        }
    }

    #[test]
    fn a_stored_scheme_that_is_neither_is_unreadable() {
        let error = scheme_from_column("ftp").expect_err("an unknown scheme");

        assert!(
            matches!(error, StorageError::Unreadable { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn every_version_round_trips_through_its_column() {
        for version in EVERY_VERSION {
            assert_eq!(version_from_column(version_column(*version)), *version);
        }
    }

    #[test]
    fn a_version_this_build_has_no_name_for_reads_back_as_unknown() {
        assert_eq!(version_from_column("HTTP/4.2"), HttpVersion::Unknown);
    }

    #[test]
    fn every_error_kind_round_trips_through_its_column() {
        for kind in EVERY_KIND {
            assert_eq!(
                error_kind_from_column(error_kind_column(*kind)).expect("a kind"),
                *kind
            );
        }
    }

    #[test]
    fn a_stored_error_kind_this_build_does_not_know_is_unreadable() {
        let error = error_kind_from_column("something_new").expect_err("an unknown kind");

        assert!(
            matches!(error, StorageError::Unreadable { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn an_error_round_trips_through_its_columns() {
        let error = TrafficError::new(
            TrafficErrorKind::UpstreamTls,
            "the origin's certificate did not verify",
        );

        let read_back = error_from_columns(
            Some(error_kind_column(error.kind()).to_owned()),
            Some(error.summary().to_owned()),
        )
        .expect("an error");

        assert_eq!(read_back, Some(error));
    }

    #[test]
    fn no_error_columns_mean_no_error() {
        assert_eq!(error_from_columns(None, None).expect("no error"), None);
    }

    #[test]
    fn an_error_kind_with_no_summary_is_unreadable() {
        let error = error_from_columns(Some("client_tls".to_owned()), None)
            .expect_err("an error with no summary");

        assert!(
            matches!(error, StorageError::Unreadable { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn headers_round_trip_through_their_column() {
        let mut headers = Headers::new();
        headers.push(
            "Set-Cookie",
            crate::headers::HeaderValue::Text("a=1".into()),
        );
        headers.push(
            "Set-Cookie",
            crate::headers::HeaderValue::Text("b=2".into()),
        );
        headers.push(
            "X-Raw",
            crate::headers::HeaderValue::from_bytes(&[0x00, 0xff]),
        );

        let json = headers_column(&headers).expect("serializes");

        assert_eq!(headers_from_column(&json).expect("deserializes"), headers);
    }

    #[test]
    fn the_same_headers_always_produce_the_same_column() {
        let mut headers = Headers::new();
        headers.push("Accept", crate::headers::HeaderValue::Text("*/*".into()));

        assert_eq!(
            headers_column(&headers).expect("serializes"),
            headers_column(&headers).expect("serializes")
        );
    }

    #[test]
    fn a_headers_column_that_is_not_the_json_it_was_is_unreadable() {
        let error = headers_from_column("not json").expect_err("unreadable headers");

        assert!(
            matches!(error, StorageError::Unreadable { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_body_read_back_keeps_its_size_and_whether_it_was_cut_short() {
        let complete = body_from_columns(b"hello".to_vec(), 5, false);
        assert_eq!(complete, BodyCapture::complete(b"hello".to_vec()));

        let truncated = body_from_columns(b"hel".to_vec(), 4_096, true);
        assert_eq!(truncated.data(), b"hel");
        assert_eq!(truncated.size(), 4_096);
        assert!(truncated.is_truncated());
    }

    #[test]
    fn every_role_round_trips_through_its_column() {
        for role in [MessageRole::Request, MessageRole::Response] {
            assert_eq!(
                MessageRole::from_column(role.as_column()).expect("a role"),
                role
            );
        }
        assert!(MessageRole::from_column("trailer").is_err());
    }

    #[test]
    fn a_client_address_round_trips_through_its_column() {
        for text in ["127.0.0.1:54321", "[::1]:8080"] {
            let address = client_from_column(text).expect("an address");

            assert_eq!(address.to_string(), text);
        }
    }

    #[test]
    fn a_client_address_that_is_not_an_address_is_unreadable() {
        let error = client_from_column("nowhere").expect_err("not an address");

        assert!(
            matches!(error, StorageError::Unreadable { .. }),
            "{error:?}"
        );
    }
}
