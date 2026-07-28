//! Single-instance protection for `devsignal run`.
//!
//! Two daemons both push presence to the same Discord client and fight over the card: whichever
//! pushed last wins until the other's next tick, so the activity flickers between two payloads and
//! each daemon's debouncer thinks its own view is what Discord is showing. Nothing detects that today
//! — a LaunchAgent copy plus a hand-run `devsignal run` is all it takes.
//!
//! `flock` rather than a pidfile: the kernel releases the lock when the holding process dies, however
//! it dies, so there is no stale-lock case to detect. A pidfile would need a `kill(pid, 0)` liveness
//! probe to survive `SIGKILL`, which means `libc` anyway.

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// Held for as long as the daemon runs. Dropping it closes the descriptor, which releases the lock.
///
/// Bind it to a named local (`let _lock = …`); `let _ = …` drops it immediately and silently disables
/// the whole mechanism.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// The lock lives beside the config it belongs to, so two daemons on *different* configs are allowed.
///
/// That is deliberate. Sharing one Discord client id is what actually conflicts, and the config is the
/// closest available proxy for it; a single global lock would also collide across the test suite's
/// temporary directories.
pub fn lock_path_for(config_path: &Path) -> PathBuf {
    // A bare filename's parent is `Some("")`, not `None`, and joining onto an empty path would put the
    // lock at a bare relative name. Normalise both cases to the working directory.
    let dir = match config_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    dir.join(".devsignal.lock")
}

fn holder_pid(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Take the exclusive lock for `config_path`, or fail explaining who holds it.
pub fn acquire(config_path: &Path) -> Result<InstanceLock> {
    let path = lock_path_for(config_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create lock directory {}", dir.display()))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open lock file {}", path.display()))?;

    // SAFETY: `file` owns a valid descriptor for the duration of the call, and `flock` only consults
    // it. LOCK_NB makes this fail immediately rather than blocking on the other daemon forever.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        let holder = holder_pid(&path)
            .map(|pid| format!("pid {pid} holds"))
            .unwrap_or_else(|| "another process holds".to_string());
        anyhow::bail!(
            "another devsignal is already running ({holder} {})\n\
             Two daemons would both push presence and fight over the Discord card. Stop the other \
             one first:\n  \
             launchctl bootout gui/$(id -u)/com.devsignal.daemon    # if it came from the LaunchAgent\n  \
             pgrep -fl devsignal                                    # to find it otherwise\n\
             (lock error: {err})",
            path.display()
        );
    }

    // Record our pid so the *next* process can name us. Best-effort: the lock is already ours, and
    // failing to write the pid only costs a less specific error message later.
    let _ = file.set_len(0);
    let _ = write!(file, "{}", std::process::id());
    let _ = file.flush();

    Ok(InstanceLock { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `flock` locks are per open-file-description, not per process, so two `acquire` calls in one
    /// process conflict exactly as two processes would. That is what makes this testable at all —
    /// `run` is macOS-gated, so a real two-process test cannot run on Linux CI.
    #[test]
    fn a_second_lock_on_the_same_config_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = dir.path().join("config.toml");

        let _first = acquire(&cfg).expect("the first lock should be granted");
        let err = acquire(&cfg).expect_err("the second lock must be refused");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("already running"),
            "the message must say what is wrong: {msg}"
        );
        assert!(
            msg.contains(&lock_path_for(&cfg).display().to_string()),
            "the message must name the lock path: {msg}"
        );
        assert!(
            msg.contains(&std::process::id().to_string()),
            "the message must name the holder's pid: {msg}"
        );
        assert!(
            msg.contains("launchctl bootout"),
            "the message must say how to stop the other daemon: {msg}"
        );
    }

    #[test]
    fn the_lock_is_released_when_the_holder_is_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = dir.path().join("config.toml");

        let first = acquire(&cfg).expect("first");
        drop(first);

        acquire(&cfg).expect("the lock must be available again once the holder is dropped");
    }

    /// Scoping the lock to the config directory is what lets someone run a second daemon against a
    /// different config on purpose — and what keeps the test suite's tempdirs from colliding.
    #[test]
    fn locks_for_different_config_dirs_do_not_collide() {
        let a = tempfile::tempdir().expect("tempdir a");
        let b = tempfile::tempdir().expect("tempdir b");

        let _one = acquire(&a.path().join("config.toml")).expect("lock a");
        let _two = acquire(&b.path().join("config.toml")).expect("lock b");
    }

    #[test]
    fn lock_path_sits_beside_the_config() {
        assert_eq!(
            lock_path_for(Path::new("/Users/demo/.config/devsignal/config.toml")),
            PathBuf::from("/Users/demo/.config/devsignal/.devsignal.lock")
        );
        // A bare filename has no parent; fall back to the working directory rather than panicking.
        assert_eq!(
            lock_path_for(Path::new("config.toml")),
            PathBuf::from("./.devsignal.lock")
        );
    }
}
