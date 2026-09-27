//! The local development certificate authority.
//!
//! Interception needs a CA the client already trusts, so this one is
//! generated once on the machine it runs on, persisted, and reused on every
//! later launch. It is never regenerated while it is there: a developer
//! installs this certificate into a trust store by hand, and quietly issuing
//! a different one would break that trust without saying so.
//!
//! What is public and what is not is kept apart deliberately.
//! [`CertificateAuthority::certificate_pem`] and
//! [`CertificateAuthority::certificate_der`] exist because the certificate is
//! meant to be handed out — that is how anything comes to trust it. There is
//! no counterpart for the private key: it is reachable only as the signing
//! key of an issuer inside this crate, is written to one file with owner-only
//! permissions, and is elided from [`fmt::Debug`].

use std::env;
use std::fmt;
use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::CertificateDer;
use time::{Duration, OffsetDateTime};

use crate::error::TlsError;
use crate::store::{self, StoredCa};

/// Subject common name of the generated CA.
///
/// It is what a developer sees in a trust store and in a client's certificate
/// viewer, so it says plainly what the certificate is and that it is for
/// development.
const COMMON_NAME: &str = "ProxyLab Development CA";

/// Subject organisation of the generated CA.
const ORGANISATION: &str = "ProxyLab";

/// How long a generated CA is valid for.
///
/// Long, because expiry means every client that trusts it has to be set up
/// again. Not unlimited, because a credential with no end is worse.
const VALIDITY_DAYS: i64 = 3650;

/// How far in the past a certificate starts being valid.
///
/// A client whose clock is a little behind this machine's would otherwise
/// reject a certificate issued seconds ago as not yet valid.
pub(crate) const CLOCK_SKEW_ALLOWANCE: Duration = Duration::hours(1);

/// Environment variable that overrides where the CA is stored.
const CA_DIRECTORY_VARIABLE: &str = "PROXYLAB_CA_DIR";

/// Directory under the home directory that holds ProxyLab's own state.
const STATE_DIRECTORY: &str = ".proxylab";

/// Directory within that which holds the CA.
const CA_DIRECTORY: &str = "ca";

/// The signature algorithm every ProxyLab certificate uses.
///
/// ECDSA with P-256 and SHA-256: a modern algorithm every current TLS client
/// accepts, and fast enough to mint a key and a certificate per intercepted
/// host without the handshake noticing.
const ALGORITHM: &rcgen::SignatureAlgorithm = &PKCS_ECDSA_P256_SHA256;

/// A certificate authority that can sign leaf certificates.
pub struct CertificateAuthority {
    certificate: CertificateDer<'static>,
    certificate_pem: String,
    issuer: Issuer<'static, KeyPair>,
}

impl CertificateAuthority {
    /// Generates a new CA, in memory.
    ///
    /// Nothing is written anywhere: this is the CA a test uses, and the one
    /// [`load_or_create`](Self::load_or_create) persists when it finds no CA
    /// already stored.
    pub fn generate() -> Result<Self, TlsError> {
        let key = KeyPair::generate_for(ALGORITHM).map_err(TlsError::Certificate)?;
        let params = params()?;
        let certificate = params.self_signed(&key).map_err(TlsError::Certificate)?;

        Ok(Self {
            certificate: certificate.der().clone(),
            certificate_pem: certificate.pem(),
            issuer: Issuer::new(params, key),
        })
    }

    /// Loads the CA stored in `directory`, generating and storing one if
    /// there is none.
    ///
    /// This is the entry point a launching proxy uses. Calling it twice, or
    /// on two later runs, yields the same CA: the stored one is loaded, and
    /// generation only happens for a directory that holds no CA at all.
    pub fn load_or_create(directory: &Path) -> Result<Self, TlsError> {
        if let Some(stored) = store::read(directory)? {
            return Self::load(directory, &stored);
        }

        let generated = Self::generate()?;
        store::write(
            directory,
            &StoredCa {
                certificate: generated.certificate_pem.clone(),
                key: generated.issuer.key().serialize_pem(),
            },
        )?;

        Ok(generated)
    }

    /// Rebuilds a CA from its stored certificate and key.
    ///
    /// The issuer is taken from the stored certificate rather than rebuilt
    /// from the constants above, so a CA written by an earlier version — with
    /// a different subject, say — still signs certificates that chain to it.
    fn load(directory: &Path, stored: &StoredCa) -> Result<Self, TlsError> {
        let key = KeyPair::from_pem(&stored.key).map_err(|source| TlsError::CaMalformed {
            path: store::key_path(directory),
            source,
        })?;
        let issuer = Issuer::from_ca_cert_pem(&stored.certificate, key).map_err(|source| {
            TlsError::CaMalformed {
                path: store::certificate_path(directory),
                source,
            }
        })?;
        let certificate = certificate_der(directory, &stored.certificate)?;

        Ok(Self {
            certificate,
            certificate_pem: stored.certificate.clone(),
            issuer,
        })
    }

    /// The CA certificate, in DER.
    ///
    /// Public material: this is what a client, a keychain, or a `curl
    /// --cacert` has to be given for interception to be accepted.
    pub fn certificate_der(&self) -> &CertificateDer<'static> {
        &self.certificate
    }

    /// The CA certificate, in PEM.
    pub fn certificate_pem(&self) -> &str {
        &self.certificate_pem
    }

    /// The issuer that signs leaf certificates.
    ///
    /// Crate-private, because it holds the private key.
    pub(crate) fn issuer(&self) -> &Issuer<'static, KeyPair> {
        &self.issuer
    }

    /// Where the CA is stored when no directory is named.
    ///
    /// `PROXYLAB_CA_DIR` overrides it outright, which is what lets a test or
    /// a second instance keep its own CA. Otherwise it is `.proxylab/ca`
    /// under the home directory: no platform's application-support
    /// conventions are assumed, because the macOS side of this is a later
    /// piece of work.
    pub fn default_directory() -> Result<PathBuf, TlsError> {
        if let Some(directory) = env::var_os(CA_DIRECTORY_VARIABLE) {
            if !directory.is_empty() {
                return Ok(PathBuf::from(directory));
            }
        }

        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .filter(|home| !home.is_empty())
            .ok_or(TlsError::NoHomeDirectory)?;

        Ok(PathBuf::from(home).join(STATE_DIRECTORY).join(CA_DIRECTORY))
    }
}

/// Never shows the private key, which is the only interesting thing it holds.
impl fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertificateAuthority")
            .field("subject", &COMMON_NAME)
            .field("certificate_bytes", &self.certificate.len())
            .field("key", &"[elided]")
            .finish()
    }
}

/// The parameters of a ProxyLab CA certificate.
///
/// The extensions are what make it usable as a CA at all:
///
/// - basic constraints with `CA=true` and a path length of zero, so it may
///   sign leaf certificates and may not sign another CA below itself;
/// - key usage including certificate signing, without which a client refuses
///   to accept it as the issuer of anything.
///
/// It carries no subject alternative name and no extended key usage: it
/// identifies no server, and an extended key usage on a root only narrows
/// what chains beneath it are allowed to be.
fn params() -> Result<CertificateParams, TlsError> {
    let mut params = CertificateParams::new(Vec::<String>::new()).map_err(TlsError::Certificate)?;
    let now = OffsetDateTime::now_utc();

    params.distinguished_name = name();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.not_before = now - CLOCK_SKEW_ALLOWANCE;
    params.not_after = now + Duration::days(VALIDITY_DAYS);
    params.use_authority_key_identifier_extension = true;

    Ok(params)
}

/// The CA's subject, which is also the issuer of every certificate it signs.
fn name() -> DistinguishedName {
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, COMMON_NAME);
    name.push(DnType::OrganizationName, ORGANISATION);

    name
}

/// Pulls the DER out of a stored PEM certificate.
///
/// The bytes are read back from the PEM rather than kept alongside it, so the
/// DER served in a TLS handshake is provably the certificate on disk.
fn certificate_der(directory: &Path, pem: &str) -> Result<CertificateDer<'static>, TlsError> {
    let malformed = || TlsError::CaMalformed {
        path: store::certificate_path(directory),
        source: rcgen::Error::CouldNotParseCertificate,
    };

    let der = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|_| malformed())?;

    Ok(der)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use crate::test_support::temp_dir;
    use x509_parser::prelude::{FromDer, X509Certificate};

    fn parsed(der: &[u8]) -> X509Certificate<'_> {
        let (rest, certificate) = X509Certificate::from_der(der).expect("valid X.509");

        assert!(rest.is_empty(), "trailing bytes after the certificate");
        certificate
    }

    #[test]
    fn a_ca_can_be_generated() {
        let ca = CertificateAuthority::generate().expect("generate a CA");

        assert!(!ca.certificate_der().is_empty());
        assert!(ca
            .certificate_pem()
            .starts_with("-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn the_generated_certificate_is_a_ca_certificate() {
        let ca = CertificateAuthority::generate().expect("generate a CA");
        let certificate = parsed(ca.certificate_der());

        let constraints = certificate
            .basic_constraints()
            .expect("basic constraints parse")
            .expect("a CA certificate has basic constraints");

        assert!(constraints.value.ca, "basic constraints do not say CA");
        assert!(constraints.critical, "basic constraints must be critical");
        assert_eq!(
            constraints.value.path_len_constraint,
            Some(0),
            "a signing-only CA should not be allowed to issue another CA"
        );
    }

    #[test]
    fn the_generated_certificate_may_sign_certificates() {
        let ca = CertificateAuthority::generate().expect("generate a CA");
        let certificate = parsed(ca.certificate_der());

        let usage = certificate
            .key_usage()
            .expect("key usage parses")
            .expect("a CA certificate has a key usage")
            .value;

        assert!(usage.key_cert_sign(), "the CA cannot sign certificates");
        assert!(usage.crl_sign(), "the CA cannot sign revocation lists");
    }

    #[test]
    fn the_generated_certificate_is_self_signed_and_names_itself() {
        let ca = CertificateAuthority::generate().expect("generate a CA");
        let certificate = parsed(ca.certificate_der());

        assert_eq!(
            certificate.subject().to_string(),
            certificate.issuer().to_string()
        );
        assert!(
            certificate.subject().to_string().contains(COMMON_NAME),
            "subject is {}",
            certificate.subject()
        );
        certificate
            .verify_signature(None)
            .expect("a self-signed certificate verifies against its own key");
    }

    #[test]
    fn the_generated_certificate_is_valid_now_and_not_forever() {
        let ca = CertificateAuthority::generate().expect("generate a CA");
        let certificate = parsed(ca.certificate_der());

        assert!(certificate.validity().is_valid(), "not valid now");

        let lifetime = certificate.validity().not_after.timestamp()
            - certificate.validity().not_before.timestamp();
        let days = lifetime / 86_400;

        assert!(
            (VALIDITY_DAYS..=VALIDITY_DAYS + 1).contains(&days),
            "lifetime is {days} days"
        );
    }

    #[test]
    fn two_generated_cas_are_different_cas() {
        let first = CertificateAuthority::generate().expect("generate a CA");
        let second = CertificateAuthority::generate().expect("generate a CA");

        assert_ne!(first.certificate_der(), second.certificate_der());
    }

    #[test]
    fn a_generated_ca_is_persisted() {
        let directory = temp_dir("ca-persist");

        let ca = CertificateAuthority::load_or_create(directory.path()).expect("create a CA");

        assert_eq!(
            fs::read_to_string(store::certificate_path(directory.path()))
                .expect("the certificate is stored"),
            ca.certificate_pem()
        );
        assert!(store::key_path(directory.path()).exists());
    }

    #[test]
    fn an_existing_ca_is_loaded_rather_than_replaced() {
        let directory = temp_dir("ca-reload");
        let first = CertificateAuthority::load_or_create(directory.path()).expect("create a CA");

        let second = CertificateAuthority::load_or_create(directory.path()).expect("load the CA");

        assert_eq!(first.certificate_der(), second.certificate_der());
        assert_eq!(first.certificate_pem(), second.certificate_pem());
    }

    #[test]
    fn reloading_does_not_rewrite_the_stored_ca() {
        let directory = temp_dir("ca-stable");
        CertificateAuthority::load_or_create(directory.path()).expect("create a CA");
        let stored_key =
            fs::read_to_string(store::key_path(directory.path())).expect("the key is stored");

        CertificateAuthority::load_or_create(directory.path()).expect("load the CA");

        assert_eq!(
            fs::read_to_string(store::key_path(directory.path())).expect("the key is still stored"),
            stored_key
        );
    }

    #[test]
    fn a_loaded_ca_still_carries_its_own_key() {
        let directory = temp_dir("ca-loaded-key");
        let first = CertificateAuthority::load_or_create(directory.path()).expect("create a CA");
        let second = CertificateAuthority::load_or_create(directory.path()).expect("load the CA");

        assert_eq!(
            first.issuer().key().serialize_der(),
            second.issuer().key().serialize_der(),
            "the loaded CA signs with a different key"
        );
    }

    #[test]
    fn an_unreadable_ca_is_reported_rather_than_replaced() {
        let directory = temp_dir("ca-malformed");
        fs::write(
            store::certificate_path(directory.path()),
            "not a certificate",
        )
        .expect("write a broken certificate");
        fs::write(store::key_path(directory.path()), "not a key").expect("write a broken key");

        let error = CertificateAuthority::load_or_create(directory.path())
            .expect_err("a broken CA is not usable");

        assert!(matches!(error, TlsError::CaMalformed { .. }), "{error}");
    }

    #[test]
    fn a_half_written_ca_is_reported_rather_than_replaced() {
        let directory = temp_dir("ca-half");
        let ca = CertificateAuthority::generate().expect("generate a CA");
        fs::write(
            store::certificate_path(directory.path()),
            ca.certificate_pem(),
        )
        .expect("write only the certificate");

        let error = CertificateAuthority::load_or_create(directory.path())
            .expect_err("half a CA is not a CA");

        assert!(matches!(error, TlsError::CaIncomplete { .. }), "{error}");
    }

    #[test]
    fn the_debug_form_does_not_carry_the_private_key() {
        let ca = CertificateAuthority::generate().expect("generate a CA");
        let key_pem = ca.issuer().key().serialize_pem();
        let key_body: String = key_pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();

        let described = format!("{ca:?}");

        assert!(!key_body.is_empty(), "the key has no body to look for");
        assert!(
            !described.contains(&key_body),
            "the debug form carries the private key"
        );
        for line in key_body.as_bytes().chunks(16) {
            let fragment = String::from_utf8(line.to_vec()).expect("base64 is text");
            assert!(
                !described.contains(&fragment),
                "the debug form carries part of the private key"
            );
        }
        assert!(described.contains("elided"), "{described}");
    }

    #[test]
    fn the_default_directory_can_be_overridden() {
        // The variable is read, not written: writing it would race with every
        // other test in this binary. What is asserted is the shape of the
        // path when it is not set, which is what a launching proxy uses.
        if env::var_os(CA_DIRECTORY_VARIABLE).is_some() {
            return;
        }

        let directory = CertificateAuthority::default_directory().expect("a home directory");

        assert!(directory.ends_with(Path::new(STATE_DIRECTORY).join(CA_DIRECTORY)));
        assert!(directory.is_absolute(), "{directory:?}");
    }
}
