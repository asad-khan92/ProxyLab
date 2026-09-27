//! Reading a cURL command as the request it describes.
//!
//! ```text
//! curl 'https://example.com/api' \
//!   -X POST \                        ──▶  POST https://example.com/api
//!   -H 'Content-Type: …' \                Content-Type: …
//!   -d '{"name":"ada"}'                   {"name":"ada"}
//! ```
//!
//! The command is [split into arguments](crate::lex) and then read as
//! arguments. Nothing is executed, no file is opened, and no environment
//! variable is read — see the lexer's documentation, which is where that
//! promise is kept.
//!
//! # Options that are understood
//!
//! | | |
//! | --- | --- |
//! | `-X`, `--request` | the method |
//! | `-H`, `--header` | a header, repeatable |
//! | `-d`, `--data`, `--data-ascii` | a body, repeatable, joined with `&` |
//! | `--data-raw` | the same, with `@` meaning nothing special |
//! | `--data-binary` | the same, sent exactly as given |
//! | `--url` | the URL |
//! | `-G`, `--get` | put the data in the query instead of the body |
//! | `--compressed` | ask for a compressed response |
//! | `-k`, `--insecure` | *reported, not applied* — see below |
//! | `-u`, `--user` | Basic authentication |
//! | `-A`, `--user-agent` | the `User-Agent` header |
//! | `-s`, `-S`, `-v`, `-i`, `-f`, `-#` and their long forms | *ignored, and reported* |
//!
//! Everything else is refused by name. An option that would change what is
//! sent — `-L`, `--data-urlencode`, `--form` — is never quietly dropped: a
//! request that is not the one the command describes is worse than an error
//! saying so.
//!
//! # What is reported rather than done
//!
//! `--insecure` cannot be honoured: ProxyLab verifies the destination
//! server's certificate on every connection it makes, and there is no way to
//! turn that off. So it is accepted and [noted](CurlNote::Insecure) rather
//! than silently ignored or turned into a refusal of an otherwise perfectly
//! good command.
//!
//! `--compressed` becomes an `Accept-Encoding` header, which is what cURL
//! sends. ProxyLab does not decompress the response, so what is captured is
//! the compressed bytes; that is [noted](CurlNote::Compressed) too.
//!
//! # Credentials
//!
//! `-u`, and credentials in a URL authority, both become an
//! `Authorization: Basic` header — the header cURL itself would send. They
//! are kept exactly and never logged. Credentials are moved *out* of the URL
//! because a URL is written to logs and shown in a traffic list, and a header
//! value is not.
//!
//! `Proxy-Authorization`, and every option that would send the request
//! through an upstream proxy, is refused: ProxyLab has no upstream proxy to
//! authenticate to, and a request that quietly stopped carrying a credential
//! would be a different request.

use std::collections::HashSet;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use proxy_http::RequestSpec;
use proxy_storage::{HeaderValue, Headers};
use serde::{Deserialize, Serialize};

use crate::error::CurlError;
use crate::lex;

/// What a cURL command came to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurlImport {
    /// The request the command describes.
    pub request: RequestSpec,
    /// What the parser has to say about it, in the order it came up.
    pub notes: Vec<CurlNote>,
}

/// Something the parser did that is worth telling a person about.
///
/// Not a warning and not an error: every one of these is a command that was
/// imported, with something about it that would otherwise be a surprise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurlNote {
    /// `--insecure` was accepted and not applied.
    Insecure,
    /// `--compressed` became an `Accept-Encoding` header.
    Compressed,
    /// Options that only change cURL's own output were ignored.
    OutputOnly,
    /// cURL's own `Content-Type` for `--data` was added.
    FormContentType,
    /// Credentials in the URL were moved into an `Authorization` header.
    CredentialsMoved,
}

impl CurlNote {
    /// What a person is told. A fixed phrase, like every other message here.
    pub fn message(self) -> &'static str {
        match self {
            Self::Insecure => {
                "--insecure was not applied: ProxyLab always verifies the destination \
                 server's certificate."
            }
            Self::Compressed => {
                "--compressed was imported as an Accept-Encoding header. ProxyLab captures \
                 the response as it arrives and does not decompress it."
            }
            Self::OutputOnly => {
                "Options that only change what cURL prints were ignored. They do not change \
                 the request."
            }
            Self::FormContentType => {
                "Content-Type: application/x-www-form-urlencoded was added, which is what \
                 cURL sends for --data."
            }
            Self::CredentialsMoved => {
                "Credentials in the URL were moved into an Authorization header, which is \
                 what cURL sends. A URL is shown in the traffic list; a header value is not."
            }
        }
    }
}

/// Reads `command` as the request it describes.
pub fn parse(command: &str) -> Result<CurlImport, CurlError> {
    let words = lex::split(command)?;
    let Some((program, arguments)) = words.split_first() else {
        return Err(CurlError::Empty);
    };

    if !names_curl(program) {
        return Err(CurlError::NotCurl);
    }

    Command::default().read(arguments)?.build()
}

/// Whether the first argument is cURL.
///
/// The last component of the path, so `/usr/bin/curl` is cURL and
/// `curl-wrapper` is not.
fn names_curl(program: &[u8]) -> bool {
    let Ok(program) = std::str::from_utf8(program) else {
        return false;
    };
    let name = program.rsplit('/').next().unwrap_or(program);

    name == "curl" || name == "curl.exe"
}

/// Where a body came from, which decides whether `@` names a file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DataKind {
    /// `-d`, `--data`, `--data-ascii`, `--data-binary`: `@` names a file.
    File,
    /// `--data-raw`: `@` is a character.
    Raw,
}

/// What has been read of a command so far.
#[derive(Default)]
struct Command {
    method: Option<String>,
    url: Option<String>,
    headers: Vec<(String, HeaderValue)>,
    data: Vec<Vec<u8>>,
    query_only: bool,
    user: Option<String>,
    user_agent: Option<Vec<u8>>,
    compressed: bool,
    insecure: bool,
    output_only: bool,
    /// Headers the command asked cURL not to send, by writing `Name:` with
    /// nothing after the colon. ProxyLab adds a few headers of cURL's own;
    /// this is how a command turns one of them off.
    removed: HashSet<String>,
}

impl Command {
    /// Reads every argument.
    fn read(mut self, arguments: &[Vec<u8>]) -> Result<Self, CurlError> {
        let mut at = 0;

        while at < arguments.len() {
            let argument = &arguments[at];
            at += 1;

            if argument.starts_with(b"--") {
                self.long(argument, arguments, &mut at)?;
            } else if argument.len() > 1 && argument[0] == b'-' {
                self.short(&argument[1..], arguments, &mut at)?;
            } else if argument == b"-" {
                // cURL's name for standard input, wherever it appears.
                return Err(CurlError::FileReference);
            } else {
                self.set_url(text(argument)?)?;
            }
        }

        Ok(self)
    }

    /// Reads one `--long` option, with or without an `=value`.
    fn long(
        &mut self,
        argument: &[u8],
        arguments: &[Vec<u8>],
        at: &mut usize,
    ) -> Result<(), CurlError> {
        let (name, inline) = match argument.iter().position(|byte| *byte == b'=') {
            Some(equals) => (
                text(&argument[..equals])?,
                Some(argument[equals + 1..].to_vec()),
            ),
            None => (text(argument)?, None),
        };
        let name = name.to_owned();

        // Every option that would send the request somewhere other than the
        // URL it names, all at once: this is one refusal and not fifteen
        // spellings of it.
        if name.starts_with("--proxy")
            || name.starts_with("--socks")
            || name == "--preproxy"
            || name == "--noproxy"
        {
            return Err(CurlError::proxy(&name));
        }

        match name.as_str() {
            "--request" => self.method = Some(text_value(&name, inline, arguments, at)?),
            "--header" => self.add_header(&value(&name, inline, arguments, at)?)?,
            "--url" => self.set_url(&text_value(&name, inline, arguments, at)?)?,
            "--user" => self.user = Some(text_value(&name, inline, arguments, at)?),
            "--user-agent" => self.user_agent = Some(value(&name, inline, arguments, at)?),

            "--data" | "--data-ascii" | "--data-binary" => {
                self.add_data(value(&name, inline, arguments, at)?, DataKind::File)?
            }
            "--data-raw" => self.add_data(value(&name, inline, arguments, at)?, DataKind::Raw)?,

            "--get" => self.query_only = true,
            "--compressed" => self.compressed = true,
            "--insecure" => self.insecure = true,

            "--silent"
            | "--show-error"
            | "--verbose"
            | "--include"
            | "--progress-bar"
            | "--fail"
            | "--no-progress-meter" => self.output_only = true,

            _ => return Err(CurlError::unsupported(&name)),
        }

        Ok(())
    }

    /// Reads a run of short options, which cURL lets be written together.
    ///
    /// `-sS` is two flags and `-XPOST` is one option and its value, so an
    /// option that takes a value takes the rest of the argument when there is
    /// any and the next argument when there is not.
    fn short(
        &mut self,
        flags: &[u8],
        arguments: &[Vec<u8>],
        at: &mut usize,
    ) -> Result<(), CurlError> {
        let mut index = 0;

        while index < flags.len() {
            let flag = flags[index];
            index += 1;

            let attached = |index: usize| -> Option<Vec<u8>> {
                (index < flags.len()).then(|| flags[index..].to_vec())
            };

            match flag {
                b'X' | b'H' | b'd' | b'u' | b'A' => {
                    let name = format!("-{}", flag as char);
                    let carried = value(&name, attached(index), arguments, at)?;

                    match flag {
                        b'X' => self.method = Some(as_text(carried)?),
                        b'H' => self.add_header(&carried)?,
                        b'd' => self.add_data(carried, DataKind::File)?,
                        b'u' => self.user = Some(as_text(carried)?),
                        _ => self.user_agent = Some(carried),
                    }

                    // The rest of the argument was the value.
                    return Ok(());
                }

                b'G' => self.query_only = true,
                b'k' => self.insecure = true,
                b's' | b'S' | b'v' | b'i' | b'f' | b'#' => self.output_only = true,
                b'x' => return Err(CurlError::proxy("-x")),

                other => {
                    return Err(CurlError::unsupported(&format!(
                        "-{}",
                        char::from_u32(u32::from(other)).unwrap_or('?')
                    )))
                }
            }
        }

        Ok(())
    }

    /// Records the URL, refusing a second one.
    fn set_url(&mut self, url: &str) -> Result<(), CurlError> {
        if self.url.is_some() {
            return Err(CurlError::MultipleUrls);
        }
        self.url = Some(url.to_owned());

        Ok(())
    }

    /// Records one `-H` header.
    ///
    /// `Name: value` is a header. `Name;` is a header with an empty value,
    /// and `Name:` with nothing after it is cURL's way of removing a header
    /// it would otherwise have added — ProxyLab adds none, so it removes
    /// nothing and adds nothing.
    fn add_header(&mut self, header: &[u8]) -> Result<(), CurlError> {
        if let Some(colon) = header.iter().position(|byte| *byte == b':') {
            let name = text(&header[..colon])?.trim().to_owned();
            let value = trimmed(&header[colon + 1..]);

            checked_name(&name)?;
            if value.is_empty() {
                self.removed.insert(name.to_ascii_lowercase());

                return Ok(());
            }

            self.headers.push((name, HeaderValue::from_bytes(value)));

            return Ok(());
        }

        if let Some(name) = header.strip_suffix(b";") {
            let name = text(name)?.trim().to_owned();
            checked_name(&name)?;
            self.headers.push((name, HeaderValue::Text(String::new())));

            return Ok(());
        }

        Err(CurlError::InvalidHeader)
    }

    /// Records one body argument.
    fn add_data(&mut self, data: Vec<u8>, kind: DataKind) -> Result<(), CurlError> {
        // `@name` is cURL reading the body out of a file, and `@-` is cURL
        // reading it from standard input. ProxyLab imports a command as text
        // and opens nothing, so this is refused rather than quietly sending
        // the literal characters — which would be a different request.
        if kind == DataKind::File && data.first() == Some(&b'@') {
            return Err(CurlError::FileReference);
        }

        self.data.push(data);

        Ok(())
    }

    /// Turns what was read into the request it describes.
    fn build(self) -> Result<CurlImport, CurlError> {
        let mut notes = Vec::new();

        let url = self.url.ok_or(CurlError::MissingUrl)?;
        let (url, url_credentials) = split_credentials(&url);

        // `--data` with `-G` is a query string, not a body: cURL puts it
        // after the `?` and sends no body at all.
        let (url, body) = if self.query_only {
            (with_query(&url, &self.data)?, Vec::new())
        } else {
            (url, joined(&self.data))
        };

        let method = match &self.method {
            Some(method) => method.clone(),
            // cURL's own default: `--data` without `-X` is a POST.
            None if !body.is_empty() || (!self.data.is_empty() && !self.query_only) => {
                "POST".to_owned()
            }
            None => "GET".to_owned(),
        };

        let mut headers = Headers::new();
        let mut named: HashSet<String> = self.removed;
        for (name, value) in self.headers {
            named.insert(name.to_ascii_lowercase());
            headers.push(name, value);
        }

        // Everything cURL would add for itself, after everything the command
        // spelled out — and never over a header the command spelled out,
        // because `-H` wins in cURL too.
        let credentials = self.user.as_deref().or(url_credentials.as_deref());
        if let Some(credentials) = credentials {
            if url_credentials.is_some() && self.user.is_none() {
                notes.push(CurlNote::CredentialsMoved);
            }
            if !named.contains("authorization") {
                headers.push("Authorization", HeaderValue::Text(basic(credentials)?));
            }
        }

        if let Some(agent) = self.user_agent {
            if !named.contains("user-agent") {
                headers.push("User-Agent", HeaderValue::from_bytes(&agent));
            }
        }

        if !body.is_empty() && !named.contains("content-type") {
            headers.push(
                "Content-Type",
                HeaderValue::Text("application/x-www-form-urlencoded".to_owned()),
            );
            notes.push(CurlNote::FormContentType);
        }

        if self.compressed {
            if !named.contains("accept-encoding") {
                headers.push(
                    "Accept-Encoding",
                    HeaderValue::Text("gzip, deflate".to_owned()),
                );
            }
            notes.push(CurlNote::Compressed);
        }

        if self.insecure {
            notes.push(CurlNote::Insecure);
        }
        if self.output_only {
            notes.push(CurlNote::OutputOnly);
        }

        Ok(CurlImport {
            request: RequestSpec::new(method, &url, headers, body)?,
            notes,
        })
    }
}

/// Splits credentials out of a URL authority.
///
/// `https://ada:secret@example.com/` is a URL cURL accepts and turns into an
/// `Authorization` header. ProxyLab does the same, and refuses to carry the
/// credentials in the URL at all — see [`proxy_http::RequestSpec`].
fn split_credentials(url: &str) -> (String, Option<String>) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_owned(), None);
    };

    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);

    // The last `@`, as cURL does: a password may contain one.
    let Some(at) = authority.rfind('@') else {
        return (url.to_owned(), None);
    };
    let (credentials, host) = authority.split_at(at);

    (
        format!("{scheme}://{}{tail}", &host[1..]),
        Some(credentials.to_owned()),
    )
}

/// Puts the data arguments in the URL's query, which is what `-G` does.
fn with_query(url: &str, data: &[Vec<u8>]) -> Result<String, CurlError> {
    if data.is_empty() {
        return Ok(url.to_owned());
    }

    let query = as_text(joined(data))?;
    // A fragment is never sent, and appending after one would put the query
    // in the wrong place. It is dropped here as well as in `RequestSpec`.
    let url = match url.split_once('#') {
        Some((before, _)) => before,
        None => url,
    };
    let separator = if url.contains('?') { '&' } else { '?' };

    Ok(format!("{url}{separator}{query}"))
}

/// Joins the data arguments the way cURL does: with `&` between them.
fn joined(data: &[Vec<u8>]) -> Vec<u8> {
    let mut body = Vec::new();

    for (index, part) in data.iter().enumerate() {
        if index > 0 {
            body.push(b'&');
        }
        body.extend_from_slice(part);
    }

    body
}

/// The `Authorization: Basic` value for `user:password`.
fn basic(credentials: &str) -> Result<String, CurlError> {
    if !credentials.contains(':') {
        // cURL would ask for the password at the terminal. ProxyLab has no
        // terminal to ask at, and inventing an empty password would send a
        // credential nobody wrote.
        return Err(CurlError::InvalidUser);
    }

    Ok(format!("Basic {}", STANDARD.encode(credentials)))
}

/// The value of an option: the `=value` it carried, or the next argument.
fn value(
    name: &str,
    inline: Option<Vec<u8>>,
    arguments: &[Vec<u8>],
    at: &mut usize,
) -> Result<Vec<u8>, CurlError> {
    if let Some(inline) = inline {
        return Ok(inline);
    }

    let Some(next) = arguments.get(*at) else {
        return Err(CurlError::missing_value(name));
    };
    *at += 1;

    Ok(next.clone())
}

/// The same, for an option whose value has to be text.
fn text_value(
    name: &str,
    inline: Option<Vec<u8>>,
    arguments: &[Vec<u8>],
    at: &mut usize,
) -> Result<String, CurlError> {
    as_text(value(name, inline, arguments, at)?)
}

/// Bytes as text, or the failure that says they are not.
fn text(bytes: &[u8]) -> Result<&str, CurlError> {
    std::str::from_utf8(bytes).map_err(|_| CurlError::NotText)
}

/// Owned bytes as text, or the failure that says they are not.
fn as_text(bytes: Vec<u8>) -> Result<String, CurlError> {
    String::from_utf8(bytes).map_err(|_| CurlError::NotText)
}

/// A header value with the optional whitespace around it removed.
fn trimmed(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |last| last + 1);

    &value[start..end]
}

/// Refuses a header name that is not one, so the failure names the header
/// rather than coming back as a failure of the whole request.
fn checked_name(name: &str) -> Result<(), CurlError> {
    let usable = !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        });

    if usable {
        Ok(())
    } else {
        Err(CurlError::InvalidHeader)
    }
}
