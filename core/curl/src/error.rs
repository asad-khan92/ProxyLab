//! Why a pasted cURL command is not one ProxyLab can send.
//!
//! Every reason is a fixed phrase written here. The one thing that is ever
//! repeated back from the command is the *name* of an unsupported option —
//! `--resolve`, `--proxy` — because "unsupported cURL option" without saying
//! which one is not an error message. It is checked before it goes anywhere:
//! short, printable ASCII, or it is left out.
//!
//! Nothing else crosses. Not a URL, not a header, not a body, not a value of
//! any kind: a cURL command is exactly where a bearer token lives, and an
//! error message is exactly the thing that gets shown, copied into a chat
//! window, and pasted into a bug report.
//!
//! # Not a failure of the request
//!
//! A cURL command that does not parse is *data* that did not parse, so this
//! is carried back as the result of a successful `parse_curl` rather than as
//! an IPC error. That keeps the IPC error messages what they have always
//! been — fixed phrases from `core/ipc/` — and lets the interface say which
//! option it was. See [`crate::CurlImport`].

use std::fmt;

use proxy_http::RequestSpecError;
use serde::{Deserialize, Serialize};

/// The longest option name that is ever repeated back.
///
/// A real option name is a handful of characters. Anything longer is not one,
/// and is left out rather than echoed.
const MAX_OPTION_LENGTH: usize = 40;

/// Why a cURL command could not be turned into a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CurlError {
    /// There was nothing to parse.
    Empty,
    /// The command does not start with `curl`.
    NotCurl,
    /// A quote was opened and never closed.
    UnterminatedQuote,
    /// The command would have the shell run something or read a variable.
    ShellExpansion,
    /// The command is a pipeline, a redirection, or more than one command.
    ShellMetacharacter,
    /// An escape inside `$'…'` names something that is not a character.
    InvalidEscape,
    /// An option this build does not implement.
    UnsupportedOption(Option<String>),
    /// An option that sends the request through an upstream proxy.
    ProxyOption(Option<String>),
    /// An option that needs a value was given none.
    MissingOptionValue(Option<String>),
    /// The command names more than one URL.
    MultipleUrls,
    /// The command names no URL.
    MissingUrl,
    /// A `-H` value is not `Name: value`.
    InvalidHeader,
    /// The command reads its body from a file, or from standard input.
    FileReference,
    /// A `--user` value is not `user:password`.
    InvalidUser,
    /// A URL, a header, or a method was given as bytes that are not text.
    NotText,
    /// The request the command describes is not one ProxyLab can send.
    Request(RequestSpecError),
}

impl CurlError {
    /// An unsupported option, named if the name is one worth repeating.
    pub(crate) fn unsupported(option: &str) -> Self {
        Self::UnsupportedOption(safe_option(option))
    }

    /// An option that would send the request through an upstream proxy.
    pub(crate) fn proxy(option: &str) -> Self {
        Self::ProxyOption(safe_option(option))
    }

    /// An option that was given no value.
    pub(crate) fn missing_value(option: &str) -> Self {
        Self::MissingOptionValue(safe_option(option))
    }

    /// The option this error is about, when it is about one.
    ///
    /// The interface puts it in the message it shows. It is the user's own
    /// text going back to the same user, and it has already been checked:
    /// see [`safe_option`].
    pub fn option(&self) -> Option<&str> {
        match self {
            Self::UnsupportedOption(option)
            | Self::ProxyOption(option)
            | Self::MissingOptionValue(option) => option.as_deref(),
            _ => None,
        }
    }

    /// A name a client matches on, which never changes for a given failure.
    pub fn code(&self) -> CurlErrorCode {
        match self {
            Self::Empty => CurlErrorCode::Empty,
            Self::NotCurl => CurlErrorCode::NotCurl,
            Self::UnterminatedQuote => CurlErrorCode::UnterminatedQuote,
            Self::ShellExpansion => CurlErrorCode::ShellExpansion,
            Self::ShellMetacharacter => CurlErrorCode::ShellMetacharacter,
            Self::InvalidEscape => CurlErrorCode::InvalidEscape,
            Self::UnsupportedOption(_) => CurlErrorCode::UnsupportedOption,
            Self::ProxyOption(_) => CurlErrorCode::ProxyOption,
            Self::MissingOptionValue(_) => CurlErrorCode::MissingOptionValue,
            Self::MultipleUrls => CurlErrorCode::MultipleUrls,
            Self::MissingUrl => CurlErrorCode::MissingUrl,
            Self::InvalidHeader => CurlErrorCode::InvalidHeader,
            Self::FileReference => CurlErrorCode::FileReference,
            Self::InvalidUser => CurlErrorCode::InvalidUser,
            Self::NotText => CurlErrorCode::NotText,
            Self::Request(_) => CurlErrorCode::InvalidRequest,
        }
    }

    /// What a person is told, without the option name.
    ///
    /// A fixed phrase in every case: the only variable part of any of these
    /// messages is the option name, which is [`option`](Self::option) and is
    /// put in by whoever is showing the message.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Empty => "paste a cURL command to import",
            Self::NotCurl => "a cURL command starts with curl",
            Self::UnterminatedQuote => "a quote in this command is never closed",
            Self::ShellExpansion => {
                "this command asks the shell to substitute something. ProxyLab reads a \
                 cURL command as text and never runs it, so put the value in directly"
            }
            Self::ShellMetacharacter => {
                "this is more than one shell command. Import the cURL command on its own"
            }
            Self::InvalidEscape => "an escape in this command does not name a character",
            Self::UnsupportedOption(_) => "ProxyLab does not implement this cURL option",
            Self::ProxyOption(_) => {
                "ProxyLab does not send requests through an upstream proxy, so it cannot \
                 import this option"
            }
            Self::MissingOptionValue(_) => "this cURL option needs a value",
            Self::MultipleUrls => "this command names more than one URL",
            Self::MissingUrl => "this command names no URL",
            Self::InvalidHeader => "a header is written Name: value",
            Self::FileReference => {
                "ProxyLab does not read files named in a cURL command. Paste the body in \
                 directly, or use --data-raw"
            }
            Self::InvalidUser => "a --user value is written user:password",
            Self::NotText => "part of this command is bytes that are not text",
            Self::Request(error) => error.reason(),
        }
    }

    /// The whole message, with the option name in it when there is one.
    pub fn message(&self) -> String {
        match self.option() {
            Some(option) => format!("{} ({option})", self.reason()),
            None => self.reason().to_owned(),
        }
    }
}

impl fmt::Display for CurlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for CurlError {}

impl From<RequestSpecError> for CurlError {
    fn from(error: RequestSpecError) -> Self {
        Self::Request(error)
    }
}

/// What kind of failure it was, for a client to match on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurlErrorCode {
    /// There was nothing to parse.
    Empty,
    /// The command does not start with `curl`.
    NotCurl,
    /// A quote was opened and never closed.
    UnterminatedQuote,
    /// The command would have the shell run something.
    ShellExpansion,
    /// The command is more than one command.
    ShellMetacharacter,
    /// An escape does not name a character.
    InvalidEscape,
    /// An option this build does not implement.
    UnsupportedOption,
    /// An option that needs an upstream proxy.
    ProxyOption,
    /// An option that needs a value was given none.
    MissingOptionValue,
    /// More than one URL.
    MultipleUrls,
    /// No URL.
    MissingUrl,
    /// A header that is not `Name: value`.
    InvalidHeader,
    /// A body read from a file.
    FileReference,
    /// A `--user` that is not `user:password`.
    InvalidUser,
    /// Bytes that are not text where text was needed.
    NotText,
    /// The request itself is not one ProxyLab can send.
    InvalidRequest,
}

/// The option name, if it is one that is safe to repeat back.
///
/// Short, and printable ASCII with no space in it — which every real option
/// name is. Anything else is dropped: the message is still accurate without
/// it, and an error message is not a channel for a client's own bytes.
fn safe_option(option: &str) -> Option<String> {
    let usable = !option.is_empty()
        && option.len() <= MAX_OPTION_LENGTH
        && option
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'\'' && byte != b'"');

    usable.then(|| option.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsupported_option_names_itself() {
        let error = CurlError::unsupported("--resolve");

        assert_eq!(error.option(), Some("--resolve"));
        assert_eq!(error.code(), CurlErrorCode::UnsupportedOption);
        assert!(error.message().contains("--resolve"), "{error}");
    }

    #[test]
    fn an_option_name_that_is_not_one_is_left_out() {
        for name in [
            "--this-is-a-very-long-option-name-that-nobody-would-ever-write",
            "--has space",
            "--has'quote",
            "",
            "--newline\n",
        ] {
            let error = CurlError::unsupported(name);

            assert_eq!(error.option(), None, "{name:?} was repeated back");
        }
    }

    #[test]
    fn no_error_message_carries_a_value_from_the_command() {
        // Every phrase is fixed and chosen here; the only variable part is
        // an option name, which is checked. This is the test that says so.
        let errors = [
            CurlError::Empty,
            CurlError::NotCurl,
            CurlError::UnterminatedQuote,
            CurlError::ShellExpansion,
            CurlError::ShellMetacharacter,
            CurlError::InvalidEscape,
            CurlError::MultipleUrls,
            CurlError::MissingUrl,
            CurlError::InvalidHeader,
            CurlError::FileReference,
            CurlError::InvalidUser,
            CurlError::NotText,
            CurlError::Request(RequestSpecError::InvalidUrl),
        ];

        for error in errors {
            assert_eq!(error.message(), error.reason());
        }
    }

    #[test]
    fn every_failure_has_a_code_of_its_own() {
        let codes = [
            CurlError::Empty.code(),
            CurlError::NotCurl.code(),
            CurlError::UnterminatedQuote.code(),
            CurlError::ShellExpansion.code(),
            CurlError::ShellMetacharacter.code(),
            CurlError::InvalidEscape.code(),
            CurlError::unsupported("-Z").code(),
            CurlError::proxy("-x").code(),
            CurlError::missing_value("-H").code(),
            CurlError::MultipleUrls.code(),
            CurlError::MissingUrl.code(),
            CurlError::InvalidHeader.code(),
            CurlError::FileReference.code(),
            CurlError::InvalidUser.code(),
            CurlError::NotText.code(),
            CurlError::Request(RequestSpecError::InvalidUrl).code(),
        ];
        let unique: std::collections::HashSet<_> = codes.iter().collect();

        assert_eq!(unique.len(), codes.len());
    }

    #[test]
    fn a_code_is_written_as_the_name_a_client_matches_on() {
        let json = serde_json::to_string(&CurlErrorCode::UnsupportedOption).expect("serializes");

        assert_eq!(json, r#""unsupported_option""#);
    }
}
