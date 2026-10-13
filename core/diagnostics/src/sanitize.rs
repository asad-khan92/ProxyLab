//! Text that is safe to print and safe to paste into an issue.
//!
//! A log line and a diagnostics report travel further than the machine they
//! were written on. What they may carry is the shape of a failure — which
//! component, which operation, what the operating system said — and not the
//! traffic that failure was about.
//!
//! The rules are applied to the finished text, so a caller that formatted a
//! URL or a header into a message still cannot leak it. Applying them twice
//! is the same as applying them once.

/// The longest message kept. A longer one is a body, or a panic payload,
/// that has no business in a diagnostic line.
const MAX_MESSAGE_CHARS: usize = 240;

/// What stands in for a secret.
const REDACTED: &str = "[redacted]";

/// What stands in for a personal filesystem path.
const PATH: &str = "[path]";

/// `text` with secrets, query strings, and personal paths removed.
pub fn sanitize(text: &str) -> String {
    let without_keys = redact_pem(text);
    let without_headers = redact_header_lines(&without_keys);
    let without_schemes = redact_prefixed(&redact_prefixed(&without_headers, "bearer "), "basic ");
    let mut out = String::with_capacity(without_schemes.len());

    for token in split_keeping_whitespace(&without_schemes) {
        if token.chars().all(char::is_whitespace) {
            out.push_str(token);
            continue;
        }

        out.push_str(&sanitize_token(token));
    }

    truncate(&out)
}

/// An endpoint a diagnostic may name: a host and an optional port, and
/// nothing that could be a URL, a header, or a credential.
pub fn endpoint(value: &str) -> Option<String> {
    let value = value.trim();

    if value.is_empty() || value.len() > 128 {
        return None;
    }

    if value.contains(['@', '?', '/', '\\', ' ', '\t', '\n', '=', ':']) && !is_host_port(value) {
        return None;
    }

    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
    {
        return None;
    }

    if !is_host_port(value) {
        return None;
    }

    Some(value.to_owned())
}

/// An exchange id a diagnostic may name. Anything else is dropped rather
/// than carried: an id is a UUID, and a field that accepted arbitrary text
/// would be a place to put a URL.
pub fn exchange_id(value: &str) -> Option<String> {
    let value = value.trim();

    if !(8..=64).contains(&value.len()) {
        return None;
    }

    if value.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        Some(value.to_owned())
    } else {
        None
    }
}

/// `host`, `host:port`, or `[ipv6]:port`.
fn is_host_port(value: &str) -> bool {
    if let Some(rest) = value.strip_prefix('[') {
        let Some((addr, port)) = rest.split_once(']') else {
            return false;
        };

        return !addr.is_empty()
            && addr.chars().all(|c| c.is_ascii_hexdigit() || c == ':')
            && (port.is_empty() || is_port(port));
    }

    match value.rsplit_once(':') {
        Some((host, port)) if is_port(port) && !host.is_empty() && !host.contains(':') => host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'),
        None => value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'),
        Some(_) => false,
    }
}

fn is_port(value: &str) -> bool {
    value
        .strip_prefix(':')
        .unwrap_or(value)
        .chars()
        .all(|c| c.is_ascii_digit())
        && !value.is_empty()
        && value != ":"
}

fn sanitize_token(token: &str) -> String {
    if token.contains("ca-key") {
        return REDACTED.to_owned();
    }

    if looks_like_path(token) {
        return PATH.to_owned();
    }

    let mut token = redact_userinfo(token);
    if let Some(query) = token.find('?') {
        token.truncate(query);
    }

    redact_assignments(&mut token);
    token
}

/// Replaces a header value through the end of its line.
///
/// The value is the part that carries the secret, and it may contain spaces
/// (`Bearer …`), so the line is the bound rather than the next word.
fn redact_header_lines(text: &str) -> String {
    const NAMES: &[&str] = &[
        "authorization:",
        "proxy-authorization:",
        "cookie:",
        "set-cookie:",
        "x-api-key:",
        "api-key:",
    ];

    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map(|body| (body, true))
            .unwrap_or((line, false));
        let lower = body.to_ascii_lowercase();
        if let Some(start) = NAMES.iter().find_map(|name| lower.find(name)) {
            let name_len = NAMES
                .iter()
                .find(|name| lower[start..].starts_with(*name))
                .map(|name| name.len())
                .unwrap_or(0);
            out.push_str(&body[..start + name_len]);
            out.push(' ');
            out.push_str(REDACTED);
        } else {
            out.push_str(body);
        }
        if newline {
            out.push('\n');
        }
    }
    out
}

/// Replaces the word after `prefix`, matched regardless of ASCII case.
fn redact_prefixed(text: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find(prefix) else {
            out.push_str(rest);
            break;
        };

        let after = start + prefix.len();
        out.push_str(&rest[..after]);
        let tail = &rest[after..];
        let skip = tail.find(char::is_whitespace).unwrap_or(tail.len());
        out.push_str(REDACTED);
        rest = &tail[skip..];
    }

    out
}

fn looks_like_path(token: &str) -> bool {
    token.starts_with("/Users/")
        || token.starts_with("/home/")
        || token.starts_with("/private/var/folders/")
        || token.starts_with("/var/folders/")
}

fn redact_userinfo(token: &str) -> String {
    let Some(scheme_end) = token.find("://") else {
        return redact_authority_userinfo(token);
    };

    let authority_start = scheme_end + 3;
    let rest = &token[authority_start..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let Some(at) = authority.rfind('@') else {
        return token.to_owned();
    };

    let mut out = String::with_capacity(token.len());
    out.push_str(&token[..authority_start]);
    out.push_str(REDACTED);
    out.push('@');
    out.push_str(&authority[at + 1..]);
    out.push_str(&rest[authority_end..]);
    out
}

fn redact_authority_userinfo(token: &str) -> String {
    let Some(at) = token.rfind('@') else {
        return token.to_owned();
    };

    let userinfo = &token[..at];
    if !userinfo.contains(':') {
        return token.to_owned();
    }

    let mut out = String::from(REDACTED);
    out.push('@');
    out.push_str(&token[at + 1..]);
    out
}

fn redact_assignments(token: &mut String) {
    let lower = token.to_ascii_lowercase();
    const KEYS: &[&str] = &[
        "password=",
        "passwd=",
        "token=",
        "secret=",
        "api_key=",
        "apikey=",
        "access_token=",
    ];

    for key in KEYS {
        if let Some(start) = lower.find(key) {
            let value = start + key.len();
            let end = token[value..]
                .find(['&', ' ', '\t'])
                .map(|offset| value + offset)
                .unwrap_or(token.len());
            token.replace_range(value..end, REDACTED);
            return;
        }
    }
}

fn redact_pem(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find("-----BEGIN") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some(end) = after.find("-----END") {
            let tail = &after[end..];
            let consume = tail
                .find('\n')
                .map(|offset| end + offset + 1)
                .unwrap_or(after.len());
            out.push_str(REDACTED);
            rest = &after[consume..];
        } else {
            out.push_str(REDACTED);
            return out;
        }
    }

    out.push_str(rest);
    out
}

fn truncate(text: &str) -> String {
    let count = text.chars().count();
    if count <= MAX_MESSAGE_CHARS {
        return text.to_owned();
    }

    let mut out: String = text.chars().take(MAX_MESSAGE_CHARS).collect();
    out.push('…');
    out
}

fn split_keeping_whitespace(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_space = text.chars().next().is_some_and(char::is_whitespace);
    let mut index = 0;

    for (offset, ch) in text.char_indices() {
        let space = ch.is_whitespace();
        if space != in_space {
            parts.push(&text[start..offset]);
            start = offset;
            in_space = space;
        }
        index = offset + ch.len_utf8();
    }

    if start < text.len() {
        parts.push(&text[start..]);
    } else if index == 0 && text.is_empty() {
        return parts;
    }

    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_in_a_url_do_not_survive() {
        let line = sanitize("upstream failed http://alice:hunter2@example.com/orders");

        assert!(!line.contains("alice"), "{line}");
        assert!(!line.contains("hunter2"), "{line}");
        assert!(line.contains("[redacted]@example.com"), "{line}");
    }

    #[test]
    fn a_query_string_is_removed() {
        let line = sanitize("GET https://example.com/search?token=secret&q=1 failed");

        assert!(!line.contains("token"), "{line}");
        assert!(!line.contains("secret"), "{line}");
        assert!(line.contains("https://example.com/search"), "{line}");
        assert!(!line.contains('?'), "{line}");
    }

    #[test]
    fn an_authorization_header_value_is_removed() {
        let line = sanitize("saw Authorization: Bearer abc.def.ghi in a message");

        assert!(!line.contains("abc.def"), "{line}");
        assert!(line.contains("[redacted]"), "{line}");
    }

    #[test]
    fn a_cookie_and_a_password_assignment_are_removed() {
        let cookie = sanitize("Cookie: session=abc123");
        let password = sanitize("login password=hunter2 failed");

        assert!(!cookie.contains("abc123"), "{cookie}");
        assert!(!password.contains("hunter2"), "{password}");
        assert!(password.contains("password=[redacted]"), "{password}");
    }

    #[test]
    fn a_private_key_block_is_removed() {
        let line = sanitize(
            "before\n-----BEGIN PRIVATE KEY-----\nMIIEsecret\n-----END PRIVATE KEY-----\nafter",
        );

        assert!(!line.contains("MIIE"), "{line}");
        assert!(!line.contains("PRIVATE KEY"), "{line}");
        assert!(line.contains("[redacted]"), "{line}");
        assert!(line.contains("before"), "{line}");
        assert!(line.contains("after"), "{line}");
    }

    #[test]
    fn a_home_directory_path_is_removed() {
        let line =
            sanitize("could not open /Users/ada/.proxylab/traffic/traffic.db: permission denied");

        assert!(!line.contains("ada"), "{line}");
        assert!(!line.contains(".proxylab"), "{line}");
        assert!(line.contains("[path]"), "{line}");
        assert!(line.contains("permission denied"), "{line}");
    }

    #[test]
    fn a_ca_key_path_is_removed() {
        let line = sanitize("read /Users/ada/.proxylab/ca/ca-key.pem");

        assert!(!line.contains("ca-key"), "{line}");
        assert!(!line.contains("ada"), "{line}");
        assert_eq!(line, "read [redacted]");
    }

    #[test]
    fn sanitizing_twice_changes_nothing() {
        let once = sanitize("http://alice:pw@example.com/a?token=1 Cookie: a=b /home/ada/x");
        assert_eq!(sanitize(&once), once);
    }

    #[test]
    fn an_ordinary_failure_is_kept() {
        let line = sanitize("connect failed: connection refused");
        assert_eq!(line, "connect failed: connection refused");
    }

    #[test]
    fn a_long_message_is_cut_off() {
        let line = sanitize(&"a".repeat(500));
        assert!(line.chars().count() <= MAX_MESSAGE_CHARS + 1, "{line}");
        assert!(line.ends_with('…'), "{line}");
    }

    #[test]
    fn an_endpoint_is_a_host_and_a_port() {
        assert_eq!(
            endpoint("example.com:443").as_deref(),
            Some("example.com:443")
        );
        assert_eq!(
            endpoint("127.0.0.1:9090").as_deref(),
            Some("127.0.0.1:9090")
        );
        assert_eq!(endpoint("[::1]:9091").as_deref(), Some("[::1]:9091"));
        assert_eq!(endpoint("https://user:pw@example.com/a?x=1"), None);
        assert_eq!(endpoint("example.com/path"), None);
        assert_eq!(endpoint(""), None);
    }

    #[test]
    fn an_exchange_id_is_hexadecimal() {
        assert_eq!(
            exchange_id("018f6b3e-7c2a-7b3d-8e4f-9a0b1c2d3e4f").as_deref(),
            Some("018f6b3e-7c2a-7b3d-8e4f-9a0b1c2d3e4f")
        );
        assert_eq!(exchange_id("https://example.com/?token=1"), None);
        assert_eq!(exchange_id("short"), None);
    }
}
