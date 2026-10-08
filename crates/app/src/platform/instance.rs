//! The single-writer lock on the data directory; port of
//! `app.requestSingleInstanceLock()` in src/main/index.ts.
//!
//! One instance only. A second launch from Finder or the Dock never gets this far
//! (LaunchServices focuses the running app), so the lock is about the vault: two
//! processes writing the same file would each overwrite the other's accounts,
//! drafts and seen ids. A second process that cannot take it exits.
//!
//! The lock is `flock(LOCK_EX | LOCK_NB)` (std's `File::try_lock` on macOS), held
//! for as long as the returned `File` lives. The kernel releases it when the
//! process ends, however it ends, so a crash never leaves a stale lock behind.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The lock file's name inside the data directory.
pub const LOCK_FILE_NAME: &str = ".reviewdeck.lock";

/// Takes the lock on `<data_dir>/.reviewdeck.lock`, creating the directory and the
/// file as needed. `Ok(Some(file))` holds it until `file` is dropped (keep it for
/// the life of the app); `Ok(None)` means another process holds it. `Err` means the
/// lock could not even be tried (an unwritable data directory, say) - the vault
/// will fail the same way, so the caller decides whether to carry on.
pub fn acquire_single_instance_lock(data_dir: &Path) -> io::Result<Option<File>> {
    std::fs::create_dir_all(data_dir)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(data_dir.join(LOCK_FILE_NAME))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let path = std::env::temp_dir()
                .join(format!("reviewdeck-instance-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_one_holder_at_a_time() {
        let dir = TempDir::new("exclusive");
        // The directory does not exist yet: it is created.
        let first = acquire_single_instance_lock(&dir.0).ok().flatten();
        assert!(first.is_some(), "the first instance takes the lock");
        assert!(dir.0.join(LOCK_FILE_NAME).is_file());

        // flock locks belong to the open file, so a second open in this same process
        // contends exactly as a second process would.
        let second = acquire_single_instance_lock(&dir.0).ok();
        assert!(
            matches!(second, Some(None)),
            "the second instance is refused"
        );

        drop(first);
        let third = acquire_single_instance_lock(&dir.0).ok().flatten();
        assert!(third.is_some(), "the lock is free once the holder lets go");
    }

    #[test]
    fn an_unusable_data_dir_is_an_error_not_a_refusal() {
        let dir = TempDir::new("blocked");
        // A file where the directory should be.
        let _ = std::fs::write(&dir.0, b"");
        assert!(acquire_single_instance_lock(&dir.0).is_err());
        let _ = std::fs::remove_file(&dir.0);
    }
}
