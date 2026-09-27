//! cURL command export and import.
//!
//! ```text
//! captured request ──▶ serialize ──▶ curl '…' -H '…' --data-raw '…'
//!                                          │
//!                                          ▼
//!                                        parse ──▶ RequestSpec ──▶ replay
//! ```
//!
//! Two directions, and one model between them.
//! [`RequestSpec`](proxy_http::RequestSpec) is what a command parses into and
//! what the proxy replays, so a command this crate reads describes a request
//! the engine can send — there is no second idea here of what a URL, a
//! method, or a header may be.
//!
//! # A cURL command is data
//!
//! Nothing here executes anything. There is no `Command`, no `Process`, no
//! shell, and no path from a parsed command to one: [`parse`] splits the text
//! into arguments and reads the arguments, and a command substitution,
//! a backtick, a variable, a pipe, or a redirection is an error rather than
//! something that happens. See [`lex`] for the whole of that promise.
//!
//! A file a command names — `--data @payload.json` — is refused for the same
//! reason. Importing a command someone else wrote must not read this
//! machine's files.
//!
//! # Credentials
//!
//! A cURL command is where credentials live: `Authorization`, `Cookie`,
//! an API key in a header of a service's own naming, a `--user`. All of them
//! are kept exactly — a reproduction that drops the credential reproduces
//! nothing — and none of them is ever written to a log. No type in this crate
//! renders a header value, a body, or a generated command through [`Debug`],
//! and no error message repeats anything from a command but the name of an
//! option it does not implement.
//!
//! What this crate does *not* do is authenticate. There is no login, no token
//! refresh, no credential store, and no discovery: a credential is a header,
//! and an expired one comes back from the server as the `401` it is.

pub mod error;
pub mod lex;
pub mod parse;
pub mod serialize;

pub use error::{CurlError, CurlErrorCode};
pub use parse::{parse, CurlImport, CurlNote};
pub use serialize::{from_captured, from_request, CurlCommand};
