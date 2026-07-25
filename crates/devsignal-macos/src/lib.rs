//! macOS frontmost-application detection for labeling IDE / terminal hosts.
//!
//! Prefers `NSWorkspace` + `NSRunningApplication` (no AppleScript per poll). Falls back to
//! AppleScript (`osascript`) if the native path returns `None` twice in a row.
//!
//! The fallback is time-boxed and rate-limited, because it is a subprocess on the daemon's only
//! thread: an `osascript` that never returns would otherwise stall the poll loop, and with it both
//! shutdown and the final "clear presence" — the loop checks its stop flag between ticks, not during
//! one.

use std::time::{Duration, Instant};

/// How long to let `osascript` run before giving up on it. Generous for a one-line AppleScript;
/// short enough that a wedged `System Events` costs one tick, not the process.
pub const OSASCRIPT_TIMEOUT: Duration = Duration::from_secs(2);

/// Backoff applied after the AppleScript fallback fails, doubling per consecutive failure.
const FALLBACK_BACKOFF_BASE: Duration = Duration::from_secs(30);
const FALLBACK_BACKOFF_MAX: Duration = Duration::from_secs(300);

/// Rate limiter for the AppleScript fallback.
///
/// Without this, a permanently failing fallback forks a process every other poll — roughly every 4
/// seconds, forever — because the original code reset the miss streak *before* calling the fallback,
/// so the fallback's own failure never fed back into the decision.
///
/// Kept free of platform code so its logic is testable on any host.
#[derive(Debug, Default)]
pub struct FallbackGate {
    consecutive_failures: u32,
    next_attempt: Option<Instant>,
}

impl FallbackGate {
    pub const fn new() -> Self {
        Self {
            consecutive_failures: 0,
            next_attempt: None,
        }
    }

    /// Whether the fallback may run now.
    pub fn should_attempt(&self, now: Instant) -> bool {
        match self.next_attempt {
            Some(t) => now >= t,
            None => true,
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.next_attempt = None;
    }

    pub fn record_failure(&mut self, now: Instant) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        // Double per failure: 30s, 60s, 120s, 240s, then capped.
        let shift = self.consecutive_failures.saturating_sub(1).min(16);
        let backoff = FALLBACK_BACKOFF_BASE
            .saturating_mul(1u32 << shift)
            .min(FALLBACK_BACKOFF_MAX);
        self.next_attempt = Some(now + backoff);
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{FallbackGate, OSASCRIPT_TIMEOUT};
    use objc2::rc::autoreleasepool;
    use objc2::rc::DefaultRetained;
    use objc2_app_kit::NSWorkspace;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tracing::{debug, warn};

    /// Consecutive misses from the native AppKit path. Clamped at the trigger threshold so it cannot
    /// wrap after a long run of failures.
    static NATIVE_MISS_STREAK: AtomicU32 = AtomicU32::new(0);
    const NATIVE_MISS_TRIGGER: u32 = 2;

    static FALLBACK_GATE: Mutex<FallbackGate> = Mutex::new(FallbackGate::new());
    /// Log the "we are on the AppleScript path" notice once, not every tick.
    static FALLBACK_NOTICE_SHOWN: AtomicU32 = AtomicU32::new(0);

    /// Query frontmost app bundle id via AppKit (thread-safe API on `NSWorkspace`).
    pub fn frontmost_bundle_id_native() -> Option<String> {
        autoreleasepool(|_| {
            let workspace = NSWorkspace::default_retained();
            let apps = workspace.runningApplications();
            for app in apps.iter() {
                if app.isActive() {
                    let bid = app.bundleIdentifier()?;
                    // `NSString` implements `Display` via objc2-foundation.
                    return Some(format!("{bid}"));
                }
            }
            None
        })
    }

    /// Fallback: AppleScript via `/usr/bin/osascript` (Automation permission may be required).
    ///
    /// Time-boxed by hand because there is no timeout in `std`, and no dependency in the tree
    /// provides one: spawn, poll `try_wait` against a deadline, then kill. The script emits a single
    /// short line, far below the pipe buffer, so reading after exit cannot deadlock.
    pub fn frontmost_bundle_id_via_osascript() -> Option<String> {
        const SCRIPT: &str = r#"tell application "System Events" to get bundle identifier of first application process whose frontmost is true"#;

        if FALLBACK_NOTICE_SHOWN.swap(1, Ordering::Relaxed) == 0 {
            warn!(
                "AppKit returned no frontmost app; falling back to AppleScript. macOS may prompt for \
                 Automation access for the app that launched devsignal (Terminal, iTerm2, …). \
                 Host labels stay blank until it is granted."
            );
        }

        let mut child = match Command::new("/usr/bin/osascript")
            .args(["-e", SCRIPT])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                warn!(error = %e, "could not spawn osascript");
                return None;
            }
        };

        let deadline = Instant::now() + OSASCRIPT_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        debug!(?status, "osascript exited non-zero");
                        return None;
                    }
                    let mut out = String::new();
                    child.stdout.take()?.read_to_string(&mut out).ok()?;
                    let trimmed = out.trim();
                    return if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    };
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        // Killing it matters more than the answer: this call blocks the poll loop,
                        // and a blocked loop cannot shut down or clear presence.
                        warn!(
                            timeout = ?OSASCRIPT_TIMEOUT,
                            "osascript did not finish; killing it. A pending Automation permission \
                             prompt is the usual cause."
                        );
                        let _ = child.kill();
                        let _ = child.wait();
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    warn!(error = %e, "waiting on osascript failed");
                    return None;
                }
            }
        }
    }

    /// Bundle id of the frontmost app, preferring native AppKit; uses AppleScript after two
    /// consecutive native misses, subject to backoff when AppleScript is also failing.
    pub fn frontmost_bundle_id() -> Option<String> {
        if let Some(id) = frontmost_bundle_id_native() {
            NATIVE_MISS_STREAK.store(0, Ordering::Relaxed);
            return Some(id);
        }

        // Note: the streak is *not* reset before trying the fallback. Resetting here was what made a
        // permanently failing fallback re-fork every other tick forever.
        let streak = NATIVE_MISS_STREAK
            .load(Ordering::Relaxed)
            .saturating_add(1)
            .min(NATIVE_MISS_TRIGGER);
        NATIVE_MISS_STREAK.store(streak, Ordering::Relaxed);
        if streak < NATIVE_MISS_TRIGGER {
            return None;
        }

        let now = Instant::now();
        {
            let gate = FALLBACK_GATE.lock().ok()?;
            if !gate.should_attempt(now) {
                return None;
            }
        }

        let result = frontmost_bundle_id_via_osascript();

        if let Ok(mut gate) = FALLBACK_GATE.lock() {
            match &result {
                Some(_) => gate.record_success(),
                None => {
                    gate.record_failure(Instant::now());
                    debug!(
                        failures = gate.consecutive_failures(),
                        "AppleScript fallback failed; backing off"
                    );
                }
            }
        }
        result
    }
}

#[cfg(target_os = "macos")]
pub use imp::*;

#[cfg(not(target_os = "macos"))]
pub fn frontmost_bundle_id() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_gate_allows_an_attempt() {
        let gate = FallbackGate::new();
        assert!(gate.should_attempt(Instant::now()));
        assert_eq!(gate.consecutive_failures(), 0);
    }

    #[test]
    fn a_failure_blocks_the_next_attempt() {
        let now = Instant::now();
        let mut gate = FallbackGate::new();
        gate.record_failure(now);

        // This is the regression: previously the next poll tick would fork osascript again.
        assert!(!gate.should_attempt(now));
        assert!(!gate.should_attempt(now + Duration::from_secs(4)));
        assert!(gate.should_attempt(now + FALLBACK_BACKOFF_BASE));
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        let now = Instant::now();
        let mut gate = FallbackGate::new();

        gate.record_failure(now);
        assert!(gate.should_attempt(now + Duration::from_secs(30)));

        gate.record_failure(now);
        assert!(!gate.should_attempt(now + Duration::from_secs(30)));
        assert!(gate.should_attempt(now + Duration::from_secs(60)));

        gate.record_failure(now);
        assert!(gate.should_attempt(now + Duration::from_secs(120)));

        // Many failures must not overflow or grow without bound.
        for _ in 0..64 {
            gate.record_failure(now);
        }
        assert!(gate.should_attempt(now + FALLBACK_BACKOFF_MAX));
        assert!(!gate.should_attempt(now + FALLBACK_BACKOFF_MAX - Duration::from_secs(1)));
    }

    #[test]
    fn success_clears_the_backoff() {
        let now = Instant::now();
        let mut gate = FallbackGate::new();
        gate.record_failure(now);
        gate.record_failure(now);
        assert!(gate.consecutive_failures() > 0);

        gate.record_success();
        assert_eq!(gate.consecutive_failures(), 0);
        assert!(gate.should_attempt(now));
    }

    /// The timeout has to be well under a typical poll interval, or a wedged `osascript` still stalls
    /// the loop for a visible stretch on every attempt.
    #[test]
    fn osascript_timeout_is_shorter_than_the_default_poll_interval() {
        assert!(OSASCRIPT_TIMEOUT <= Duration::from_secs(2));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn off_macos_detection_is_a_no_op() {
        assert!(frontmost_bundle_id().is_none());
    }
}
