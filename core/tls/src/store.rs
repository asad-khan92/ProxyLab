//! Where the development CA lives on disk.
//!
//! Two PEM files in one directory: the certificate, which is public and is
//! meant to be handed to whatever needs to trust it, and the private key,
//! which is not. The distinction is the whole job of this module, so it is
//! the only place in the crate that touches the filesystem.
//!
//! The key is created with owner-only permissions, and the directory with
//! owner-only permissions too, on the platforms that have them. Neither file
//! is ever overwritten: a CA that already exists is loaded, never replaced,
//! because replacing it would silently retire the CA a developer installed
//! and trusted.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::TlsError;

/// File the CA certificate is stored in. Public material.
pub(crate) const CERTIFICATE_FILE: &str = "ca-cert.pem";

/// File the CA private key is stored in. Never leaves the machine.
pub(crate) const KEY_FILE: &str = "ca-key.pem";

/// Permissions for the private key: readable and writable by its owner only.
#[cfg(unix)]
const KEY_MODE: u32 = 0o600;

/// Permissions for the directory: enterable by its owner only, so the key is
/// not reachable even if its own mode is later loosened.
#[cfg(unix)]
const DIRECTORY_MODE: u32 = 0o700;

/// A CA as it is stored: two PEM documents.
///
/// One of them is a private key, so this type is never derived `Debug`: see
/// the implementation below.
pub(crate) struct StoredCa {
    /// The certificate, in PEM.
    pub(crate) certificate: String,
    /// The private key, in PKCS#8 PEM.
    pub(crate) key: String,
}

/// Shows that a CA was read, and not the key it was read with.
impl fmt::Debug for StoredCa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredCa")
            .field("certificate_bytes", &self.certificate.len())
            .field("key", &"[elided]")
            .finish()
    }
}

/// The path of the stored certificate in `directory`.
pub(crate) fn certificate_path(directory: &Path) -> PathBuf {
    directory.join(CERTIFICATE_FILE)
}

/// The path of the stored private key in `directory`.
pub(crate) fn key_path(directory: &Path) -> PathBuf {
    directory.join(KEY_FILE)
}

/// Reads the CA stored in `directory`.
///
/// `None` means there is nothing stored there yet, which is the caller's cue
/// to generate one. A directory holding one file and not the other is an
/// error rather than a `None`: half a CA cannot sign, and treating it as
/// absent would generate a replacement and retire a CA that may be installed
/// and trusted elsewhere.
pub(crate) fn read(directory: &Path) -> Result<Option<StoredCa>, TlsError> {
    let certificate_path = certificate_path(directory);
    let key_path = key_path(directory);

    match (certificate_path.exists(), key_path.exists()) {
        (false, false) => Ok(None),
        (true, false) => Err(TlsError::CaIncomplete {
            present: certificate_path,
            missing: key_path,
        }),
        (false, true) => Err(TlsError::CaIncomplete {
            present: key_path,
            missing: certificate_path,
        }),
        (true, true) => Ok(Some(StoredCa {
            certificate: read_file(&certificate_path)?,
            key: read_file(&key_path)?,
        })),
    }
}

/// Writes a newly generated CA into `directory`.
///
/// Both files are created exclusively, so an existing CA is never clobbered:
/// if one is already there the write fails rather than replacing it.
pub(crate) fn write(directory: &Path, ca: &StoredCa) -> Result<(), TlsError> {
    create_directory(directory)?;

    // The key first. If the process dies between the two writes, the
    // directory holds one file and not the other, which `read` reports rather
    // than papering over.
    write_file(&key_path(directory), &ca.key, private())?;
    write_file(&certificate_path(directory), &ca.certificate, public())
}

/// Creates `directory` and any parent it needs, owner-only where supported.
fn create_directory(directory: &Path) -> Result<(), TlsError> {
    let mut options = fs::DirBuilder::new();
    options.recursive(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        options.mode(DIRECTORY_MODE);
    }

    options.create(directory).map_err(|source| {
        // `recursive` makes an existing directory a success, so this is a
        // real failure: no permission, or a file in the way.
        TlsError::CaDirectory {
            path: directory.to_owned(),
            source,
        }
    })
}

/// How the private key is created: owner-only, where the platform has modes.
fn private() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(KEY_MODE);
    }

    options
}

/// How the certificate is created: it is public material, so the platform's
/// default permissions are right for it.
fn public() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    options
}

fn read_file(path: &Path) -> Result<String, TlsError> {
    fs::read_to_string(path).map_err(|source| TlsError::CaRead {
        path: path.to_owned(),
        source,
    })
}

fn write_file(path: &Path, contents: &str, options: OpenOptions) -> Result<(), TlsError> {
    let failed = |source| TlsError::CaWrite {
        path: path.to_owned(),
        source,
    };

    let mut file = options.open(path).map_err(failed)?;
    file.write_all(contents.as_bytes()).map_err(failed)?;
    file.flush().map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::temp_dir;

    fn stored() -> StoredCa {
        StoredCa {
            certificate: "-----BEGIN CERTIFICATE-----\ncert\n-----END CERTIFICATE-----\n"
                .to_owned(),
            key: "-----BEGIN PRIVATE KEY-----\nkey\n-----END PRIVATE KEY-----\n".to_owned(),
        }
    }

    #[test]
    fn the_debug_form_of_a_stored_ca_does_not_carry_the_key() {
        // This is the one type in the crate that holds a private key as
        // text, so it is the one most likely to end up in a log line.
        let described = format!("{:?}", stored());

        assert!(!described.contains("key\n-----END"), "{described}");
        assert!(!described.contains("BEGIN PRIVATE KEY"), "{described}");
        assert!(described.contains("elided"), "{described}");
    }

    #[test]
    fn an_empty_directory_holds_no_ca() {
        let directory = temp_dir("store-empty");

        assert!(read(directory.path())
            .expect("an empty directory is readable")
            .is_none());
    }

    #[test]
    fn a_directory_that_does_not_exist_holds_no_ca() {
        let directory = temp_dir("store-absent");
        let missing = directory.path().join("not-created-yet");

        assert!(read(&missing)
            .expect("an absent directory is readable")
            .is_none());
    }

    #[test]
    fn what_was_written_is_what_is_read_back() {
        let directory = temp_dir("store-roundtrip");
        let ca = stored();

        write(directory.path(), &ca).expect("write the CA");
        let read_back = read(directory.path())
            .expect("read the CA")
            .expect("the CA is there");

        assert_eq!(read_back.certificate, ca.certificate);
        assert_eq!(read_back.key, ca.key);
    }

    #[test]
    fn writing_creates_the_directory_it_needs() {
        let directory = temp_dir("store-create");
        let nested = directory.path().join("proxylab").join("ca");

        write(&nested, &stored()).expect("write the CA");

        assert!(certificate_path(&nested).exists());
        assert!(key_path(&nested).exists());
    }

    #[test]
    fn an_existing_ca_is_never_overwritten() {
        let directory = temp_dir("store-no-clobber");
        write(directory.path(), &stored()).expect("write the CA");

        let error = write(directory.path(), &stored()).expect_err("the CA is already there");

        assert!(matches!(error, TlsError::CaWrite { .. }), "{error}");
        assert_eq!(
            read(directory.path())
                .expect("read the CA")
                .expect("the CA is there")
                .key,
            stored().key,
            "the stored key was replaced"
        );
    }

    #[test]
    fn a_certificate_without_its_key_is_an_incomplete_ca() {
        let directory = temp_dir("store-half-cert");
        fs::write(certificate_path(directory.path()), "cert").expect("write a stray certificate");

        let error = read(directory.path()).expect_err("half a CA is not a CA");

        assert!(matches!(error, TlsError::CaIncomplete { .. }), "{error}");
    }

    #[test]
    fn a_key_without_its_certificate_is_an_incomplete_ca() {
        let directory = temp_dir("store-half-key");
        fs::write(key_path(directory.path()), "key").expect("write a stray key");

        let error = read(directory.path()).expect_err("half a CA is not a CA");

        assert!(matches!(error, TlsError::CaIncomplete { .. }), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn the_private_key_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = temp_dir("store-key-mode");
        write(directory.path(), &stored()).expect("write the CA");

        let mode = fs::metadata(key_path(directory.path()))
            .expect("the key is there")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, KEY_MODE, "key mode is {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn the_directory_is_enterable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = temp_dir("store-dir-mode");
        let nested = directory.path().join("ca");
        write(&nested, &stored()).expect("write the CA");

        let mode = fs::metadata(&nested)
            .expect("the directory is there")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, DIRECTORY_MODE, "directory mode is {mode:o}");
    }
}
