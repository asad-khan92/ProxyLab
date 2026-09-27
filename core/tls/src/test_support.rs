//! Test-only helpers for the crate's own unit tests.
//!
//! Only the CA-on-disk tests need anything: a directory of their own that
//! goes away afterwards. Everything else in the crate is generated in memory.
//! Hand-rolled rather than pulled from a crate, because what is needed is a
//! unique path and a `Drop` that removes it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes directories created within one test binary, since several
/// tests run at the same time in the same process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A directory that removes itself and everything in it when dropped.
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// The directory's path. It exists by the time this returns.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // A directory left behind would only be noise in the system temp
        // directory, so a failure here is not worth failing a test over.
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Creates an empty directory named after `label`, unique to this test.
pub(crate) fn temp_dir(label: &str) -> TempDir {
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "proxylab-tls-{label}-{}-{unique}",
        std::process::id()
    ));

    fs::create_dir_all(&path).expect("create the test directory");

    TempDir { path }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_exists_and_is_empty() {
        let directory = temp_dir("selftest-empty");

        assert!(directory.path().is_dir());
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("the directory is readable")
                .count(),
            0
        );
    }

    #[test]
    fn two_directories_never_collide() {
        let first = temp_dir("selftest-unique");
        let second = temp_dir("selftest-unique");

        assert_ne!(first.path(), second.path());
    }

    #[test]
    fn dropping_it_takes_its_contents_with_it() {
        let path = {
            let directory = temp_dir("selftest-drop");
            fs::write(directory.path().join("file"), "contents").expect("write a file");
            directory.path().to_owned()
        };

        assert!(!path.exists());
    }
}
