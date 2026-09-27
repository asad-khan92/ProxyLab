//! The host an intercepted TLS session claims to be.
//!
//! A `CONNECT` request names its destination as text — `example.com:443`,
//! `127.0.0.1:8443` — and that text has to serve two purposes at once: it is
//! the SNI sent to the origin server, and it is the identity written into the
//! leaf certificate presented to the client. Both must agree, and both have
//! rules the text may not satisfy.
//!
//! [`Host`] is the gate. It is built once, from client-supplied text, and
//! everything downstream takes a `Host` rather than a string, so nothing has
//! to ask again whether the thing it was handed is a usable server name.
//!
//! An IP address is not a DNS name and is not carried in SNI, so the two are
//! told apart here and stay told apart: an address becomes an IP subject
//! alternative name, a name becomes a DNS one.

use std::fmt;
use std::net::IpAddr;

use rustls_pki_types::{DnsName, ServerName};

/// A host an intercepted TLS session can be established for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Host {
    /// The host in its canonical form: a lowercased DNS name, or an IP
    /// address as its own formatter writes it.
    name: String,
    kind: Kind,
}

/// Which sort of identity a host is, which decides how it is used.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Kind {
    /// A DNS name, which is sent in SNI and becomes a DNS SAN.
    Dns(DnsName<'static>),
    /// An IP address, which is not sent in SNI and becomes an IP SAN.
    Ip(IpAddr),
}

impl Host {
    /// Reads a host out of the text a `CONNECT` target named.
    ///
    /// An IP address is recognised first, because an address written in
    /// dotted-quad form is also a syntactically plausible DNS name, and
    /// treating `127.0.0.1` as a name would put it in a DNS SAN — where no
    /// client looking for an address will find it.
    ///
    /// A DNS name is lowercased, so `EXAMPLE.com` and `example.com` are the
    /// same host and share one certificate.
    pub fn parse(host: &str) -> Result<Self, HostError> {
        if let Ok(address) = host.parse::<IpAddr>() {
            return Ok(Self {
                name: address.to_string(),
                kind: Kind::Ip(address),
            });
        }

        let name = host.to_ascii_lowercase();
        let dns = DnsName::try_from(name.clone()).map_err(|_| HostError::Unusable)?;

        Ok(Self {
            name,
            kind: Kind::Dns(dns),
        })
    }

    /// The host in the canonical form used for logs and as a cache key.
    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// Whether this host is an IP address rather than a DNS name.
    pub fn is_address(&self) -> bool {
        matches!(self.kind, Kind::Ip(_))
    }

    /// The server name to verify the origin's certificate against.
    ///
    /// For a DNS name this is also what is sent as SNI. For an IP address
    /// nothing is sent — SNI carries names only — and the address is matched
    /// against the certificate's IP subject alternative names instead.
    pub fn server_name(&self) -> ServerName<'static> {
        match &self.kind {
            Kind::Dns(name) => ServerName::DnsName(name.clone()),
            Kind::Ip(address) => ServerName::IpAddress((*address).into()),
        }
    }

    /// The subject alternative name a certificate for this host must carry.
    pub(crate) fn subject_alt_name(&self) -> Result<rcgen::SanType, HostError> {
        match &self.kind {
            Kind::Dns(name) => rcgen::string::Ia5String::try_from(name.as_ref().to_owned())
                .map(rcgen::SanType::DnsName)
                .map_err(|_| HostError::Unusable),
            Kind::Ip(address) => Ok(rcgen::SanType::IpAddress(*address)),
        }
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// Why a host cannot have a TLS session intercepted for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    /// The text is neither an IP address nor a usable DNS name, so no
    /// certificate can name it and no SNI can carry it.
    Unusable,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unusable => f.write_str("not a host name a certificate can be issued for"),
        }
    }
}

impl std::error::Error for HostError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(text: &str) -> Host {
        Host::parse(text).expect("usable host")
    }

    #[test]
    fn a_dns_name_is_a_dns_name() {
        let host = host("example.com");

        assert!(!host.is_address());
        assert_eq!(host.as_str(), "example.com");
        assert!(matches!(host.server_name(), ServerName::DnsName(_)));
    }

    #[test]
    fn a_dns_name_is_lowercased_so_one_certificate_serves_both_spellings() {
        assert_eq!(host("EXAMPLE.COM"), host("example.com"));
        assert_eq!(host("Example.Com").as_str(), "example.com");
    }

    #[test]
    fn a_subdomain_is_its_own_host() {
        assert_ne!(host("www.example.com"), host("example.com"));
    }

    #[test]
    fn an_ipv4_address_is_an_address_and_not_a_name() {
        let host = host("127.0.0.1");

        assert!(host.is_address());
        assert_eq!(host.as_str(), "127.0.0.1");
        assert!(matches!(host.server_name(), ServerName::IpAddress(_)));
    }

    #[test]
    fn an_ipv6_address_is_an_address_and_not_a_name() {
        let host = host("::1");

        assert!(host.is_address());
        assert!(matches!(host.server_name(), ServerName::IpAddress(_)));
    }

    #[test]
    fn an_address_asks_for_an_ip_subject_alt_name() {
        let san = host("127.0.0.1")
            .subject_alt_name()
            .expect("an address can be named");

        assert!(
            matches!(san, rcgen::SanType::IpAddress(_)),
            "an address must not go in a DNS SAN: {san:?}"
        );
    }

    #[test]
    fn a_name_asks_for_a_dns_subject_alt_name() {
        let san = host("example.com")
            .subject_alt_name()
            .expect("a name can be named");

        assert!(matches!(san, rcgen::SanType::DnsName(_)), "{san:?}");
    }

    #[test]
    fn localhost_is_a_name_like_any_other() {
        let host = host("localhost");

        assert!(!host.is_address());
        assert_eq!(host.as_str(), "localhost");
    }

    #[test]
    fn text_that_is_neither_a_name_nor_an_address_is_refused() {
        for text in ["", " ", "exa mple.com", "example..com", "-example.com", "*"] {
            assert_eq!(
                Host::parse(text),
                Err(HostError::Unusable),
                "{text:?} should not be a host"
            );
        }
    }

    #[test]
    fn the_rejection_explains_itself() {
        assert!(!HostError::Unusable.to_string().is_empty());
    }
}
