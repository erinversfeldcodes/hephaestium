//! Saying who is running a run directory.
//!
//! The process running a directory holds an exclusive lock on a file in it. The
//! operating system drops the lock when that process ends, however it ends, so a
//! directory nobody holds is one whose run has finished or whose runner died. A
//! sweep for leftover processes stops them only under such directories, and so
//! never takes another worker's live run, on the same machine, for a finished one.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::Path;

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

/// The lock file's name, inside the run directory.
const NAME: &str = "run.lock";

/// A run directory held by this process, until this is dropped or the process ends.
#[derive(Debug)]
pub struct Held {
    _file: File,
}

/// Take the run directory, unless another holds it.
///
/// # Errors
///
/// Returns why the lock file could not be made, or that someone else holds it.
pub fn hold(directory: &Path) -> Result<Held, String> {
    let path = directory.join(NAME);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Held { _file: file }),
        Err(Errno::WOULDBLOCK) => Err(format!("{} is held by another run", directory.display())),
        Err(e) => Err(format!("could not lock {}: {e}", path.display())),
    }
}

/// Whether some process holds the run directory. A directory with no lock file, or one that
/// cannot be inspected, is taken to be held by nobody only when it is plainly absent; any
/// other doubt counts as held, since stopping a live run's process cannot be undone.
#[must_use]
pub fn is_held(directory: &Path) -> bool {
    let file = match File::open(directory.join(NAME)) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    match flock(&file, FlockOperation::NonBlockingLockShared) {
        Ok(()) => false,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("heph-lock-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("a directory");
        path
    }

    #[test]
    fn a_directory_is_held_while_the_lock_lives_and_free_after() {
        let dir = directory("held");
        assert!(!is_held(&dir), "no lock file yet");
        let held = hold(&dir).expect("held");
        assert!(is_held(&dir));
        assert!(hold(&dir).is_err(), "a second holder is refused");
        drop(held);
        // A test running beside this one may be between fork and exec, a child that holds
        // this open file for that moment, and a lock lives as long as any holder of the
        // open file, so the release is waited for.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while is_held(&dir) {
            assert!(
                std::time::Instant::now() < deadline,
                "the directory stays held after its holder let go"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
