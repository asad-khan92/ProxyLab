//! The certificate ProxyLab presents to an intercepted client.
//!
//! One per host, minted on demand and signed by the development CA. A client
//! that trusts the CA accepts it; a client that does not refuses it, which is
//! the point — interception is visible, not silent.
//!
//! The identity is carried in the subject alternative name, because that is
//! where every current TLS client looks. The common name is filled in too,
//! for a certificate viewer to have something readable to show, but nothing
//! depends on it: a client matching only the common name would be a client
//! ignoring twenty years of certificate practice.
//!
//! Each leaf gets its own freshly generated key pair. Sharing one key across
//! hosts would make the whole set of intercepted sessions rest on a single
//! secret, and generating a P-256 key takes less time than the handshake it
//! is generated for.

use std::fmt;

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use time::{Duration, OffsetDateTime};

use crate::authority::{CertificateAuthority, CLOCK_SKEW_ALLOWANCE};
use crate::error::TlsError;
use crate::host::Host;

/// How long a leaf certificate is valid for.
///
/// A year: long enough that a developer is not watching certificates expire
/// mid-session, short enough to stay well inside the validity limits clients
/// enforce on server certificates.
const VALIDITY_DAYS: i64 = 365;

/// The signature algorithm leaf certificates use.
///
/// The same as the CA's: ECDSA with P-256 and SHA-256.
const ALGORITHM: &rcgen::SignatureAlgorithm = &rcgen::PKCS_ECDSA_P256_SHA256;

/// A certificate and the key that goes with it.
///
/// The key is private to this crate. There is no accessor for it, it is
/// elided from [`fmt::Debug`], and the only thing that reads it is the TLS
/// server configuration built from this leaf.
pub(crate) struct Leaf {
    certificate: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Leaf {
    /// Mints a certificate for `host`, signed by `ca`.
    pub(crate) fn issue(host: &Host, ca: &CertificateAuthority) -> Result<Self, TlsError> {
        let key = KeyPair::generate_for(ALGORITHM).map_err(TlsError::Certificate)?;
        let certificate = params(host)?
            .signed_by(&key, ca.issuer())
            .map_err(TlsError::Certificate)?;

        Ok(Self {
            certificate: certificate.der().clone(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        })
    }

    /// The certificate, which is what the client is sent and may inspect.
    pub(crate) fn certificate(&self) -> &CertificateDer<'static> {
        &self.certificate
    }

    /// The certificate and key, in the form a TLS server configuration wants.
    ///
    /// The chain is the leaf alone. The CA is deliberately not sent with it:
    /// a client that trusts this CA has the certificate already, and sending
    /// a path-length-constrained root as though it were an intermediate is
    /// how chain building goes wrong.
    pub(crate) fn identity(&self) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        (vec![self.certificate.clone()], self.key.clone_key())
    }
}

/// Never shows the private key, which is the only secret it holds.
impl fmt::Debug for Leaf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Leaf")
            .field("certificate_bytes", &self.certificate.len())
            .field("key", &"[elided]")
            .finish()
    }
}

/// The parameters of a leaf certificate for `host`.
///
/// The extensions are what make it usable as a server certificate:
///
/// - a subject alternative name for the host, of the right sort — a DNS name
///   for a name and an IP address for an address;
/// - an extended key usage of server authentication, without which a client
///   refuses to use it for a TLS server;
/// - basic constraints saying explicitly that this is not a CA, so a leaf
///   cannot be used to sign anything further.
fn params(host: &Host) -> Result<CertificateParams, TlsError> {
    let mut params = CertificateParams::new(Vec::<String>::new()).map_err(TlsError::Certificate)?;
    let now = OffsetDateTime::now_utc();

    params.subject_alt_names = vec![host.subject_alt_name().map_err(|_| {
        // `Host` has already been through its own validation, so this is a
        // name that is a host but cannot be written into a certificate.
        TlsError::Certificate(rcgen::Error::CouldNotParseCertificate)
    })?];
    params.distinguished_name = name(host);
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = now - CLOCK_SKEW_ALLOWANCE;
    params.not_after = now + Duration::days(VALIDITY_DAYS);
    params.use_authority_key_identifier_extension = true;

    Ok(params)
}

/// The leaf's subject: the host, for a human reading a certificate viewer.
fn name(host: &Host) -> DistinguishedName {
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, host.as_str());

    name
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::IpAddr;

    use x509_parser::extensions::GeneralName;
    use x509_parser::prelude::{FromDer, X509Certificate};

    fn host(text: &str) -> Host {
        Host::parse(text).expect("usable host")
    }

    fn authority() -> CertificateAuthority {
        CertificateAuthority::generate().expect("generate a CA")
    }

    fn issued(text: &str, ca: &CertificateAuthority) -> Leaf {
        Leaf::issue(&host(text), ca).expect("issue a leaf certificate")
    }

    fn parsed(der: &[u8]) -> X509Certificate<'_> {
        let (rest, certificate) = X509Certificate::from_der(der).expect("valid X.509");

        assert!(rest.is_empty(), "trailing bytes after the certificate");
        certificate
    }

    fn dns_names(certificate: &X509Certificate<'_>) -> Vec<String> {
        subject_alt_names(certificate)
            .into_iter()
            .filter_map(|name| match name {
                GeneralName::DNSName(name) => Some(name.to_owned()),
                _ => None,
            })
            .collect()
    }

    fn addresses(certificate: &X509Certificate<'_>) -> Vec<IpAddr> {
        subject_alt_names(certificate)
            .into_iter()
            .filter_map(|name| match name {
                GeneralName::IPAddress(bytes) => match bytes.len() {
                    4 => Some(IpAddr::from(
                        <[u8; 4]>::try_from(bytes).expect("four bytes"),
                    )),
                    16 => Some(IpAddr::from(
                        <[u8; 16]>::try_from(bytes).expect("sixteen bytes"),
                    )),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn subject_alt_names<'a>(certificate: &'a X509Certificate<'a>) -> Vec<GeneralName<'a>> {
        certificate
            .subject_alternative_name()
            .expect("subject alternative names parse")
            .map(|extension| extension.value.general_names.clone())
            .unwrap_or_default()
    }

    #[test]
    fn a_certificate_is_issued_for_a_host() {
        let ca = authority();
        let leaf = issued("example.com", &ca);

        assert!(!leaf.certificate().is_empty());
    }

    #[test]
    fn the_host_is_in_the_subject_alternative_name() {
        let ca = authority();
        let leaf = issued("example.com", &ca);
        let certificate = parsed(leaf.certificate());

        assert_eq!(dns_names(&certificate), ["example.com"]);
    }

    #[test]
    fn the_subject_alternative_name_is_the_only_identity_that_matters_but_the_subject_is_filled_in()
    {
        let ca = authority();
        let leaf = issued("www.example.com", &ca);
        let certificate = parsed(leaf.certificate());

        assert_eq!(dns_names(&certificate), ["www.example.com"]);
        assert!(
            certificate
                .subject()
                .to_string()
                .contains("www.example.com"),
            "subject is {}",
            certificate.subject()
        );
    }

    #[test]
    fn the_certificate_is_signed_by_the_ca() {
        let ca = authority();
        let leaf = issued("example.com", &ca);
        let certificate = parsed(leaf.certificate());
        let ca_certificate = parsed(ca.certificate_der());

        assert_eq!(
            certificate.issuer().to_string(),
            ca_certificate.subject().to_string()
        );
        certificate
            .verify_signature(Some(ca_certificate.public_key()))
            .expect("the leaf verifies against the CA's key");
    }

    #[test]
    fn a_certificate_from_another_ca_does_not_verify_against_this_one() {
        let ca = authority();
        let other = authority();
        let leaf = issued("example.com", &other);
        let certificate = parsed(leaf.certificate());

        certificate
            .verify_signature(Some(parsed(ca.certificate_der()).public_key()))
            .expect_err("a leaf signed by another CA must not verify");
    }

    #[test]
    fn the_certificate_is_for_server_authentication() {
        let ca = authority();
        let leaf = issued("example.com", &ca);
        let certificate = parsed(leaf.certificate());

        let usage = certificate
            .extended_key_usage()
            .expect("extended key usage parses")
            .expect("a server certificate has an extended key usage")
            .value;

        assert!(usage.server_auth, "not usable for server authentication");
        assert!(!usage.client_auth, "should not be a client certificate");
    }

    #[test]
    fn the_certificate_may_not_sign_other_certificates() {
        let ca = authority();
        let leaf = issued("example.com", &ca);
        let certificate = parsed(leaf.certificate());

        let constraints = certificate
            .basic_constraints()
            .expect("basic constraints parse")
            .expect("a leaf says explicitly that it is not a CA")
            .value;

        assert!(!constraints.ca, "a leaf must not be a CA");
    }

    #[test]
    fn the_certificate_is_valid_now_and_for_about_a_year() {
        let ca = authority();
        let leaf = issued("example.com", &ca);
        let certificate = parsed(leaf.certificate());

        assert!(certificate.validity().is_valid(), "not valid now");

        let days = (certificate.validity().not_after.timestamp()
            - certificate.validity().not_before.timestamp())
            / 86_400;

        assert!(
            (VALIDITY_DAYS..=VALIDITY_DAYS + 1).contains(&days),
            "lifetime is {days} days"
        );
    }

    #[test]
    fn different_hosts_get_different_certificates() {
        let ca = authority();

        let first = issued("example.com", &ca);
        let second = issued("other.example.com", &ca);

        assert_ne!(first.certificate(), second.certificate());
        assert_eq!(dns_names(&parsed(first.certificate())), ["example.com"]);
        assert_eq!(
            dns_names(&parsed(second.certificate())),
            ["other.example.com"]
        );
    }

    #[test]
    fn every_certificate_gets_its_own_key() {
        let ca = authority();

        let first = issued("example.com", &ca);
        let second = issued("example.com", &ca);

        let key_of = |leaf: &Leaf| parsed(leaf.certificate()).public_key().raw.to_vec();

        assert_ne!(key_of(&first), key_of(&second), "two leaves share a key");
    }

    #[test]
    fn an_ipv4_address_goes_in_an_ip_subject_alternative_name() {
        let ca = authority();
        let leaf = issued("127.0.0.1", &ca);
        let certificate = parsed(leaf.certificate());

        assert_eq!(
            addresses(&certificate),
            ["127.0.0.1".parse::<IpAddr>().expect("an address")]
        );
        assert!(
            dns_names(&certificate).is_empty(),
            "an address must not be written as a DNS name"
        );
    }

    #[test]
    fn an_ipv6_address_goes_in_an_ip_subject_alternative_name() {
        let ca = authority();
        let leaf = issued("::1", &ca);
        let certificate = parsed(leaf.certificate());

        assert_eq!(
            addresses(&certificate),
            ["::1".parse::<IpAddr>().expect("an address")]
        );
        assert!(dns_names(&certificate).is_empty());
    }

    #[test]
    fn the_debug_form_does_not_carry_the_private_key() {
        let ca = authority();
        let leaf = issued("example.com", &ca);
        let (_, key) = leaf.identity();

        let described = format!("{leaf:?}");

        for fragment in key.secret_der().chunks(8) {
            assert!(
                !described.contains(&format!("{fragment:?}")),
                "the debug form carries part of the private key"
            );
        }
        assert!(described.contains("elided"), "{described}");
    }
}
