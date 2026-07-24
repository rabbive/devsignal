//! Shutdown behaviour of the poll loop.
//!
//! This is the regression test for the release blocker: `ctrlc` was declared without the
//! `termination` feature, so only SIGINT was trapped. `launchctl bootout` and `kickstart -k` — what
//! `devsignal init` installs — send SIGTERM, so the daemon died without clearing presence and left a
//! stale activity in Discord.
//!
//! Asserting the real thing would need a Discord desktop client and macOS. `devsignal watch` runs the
//! same poll loop against a stdout sink instead, so the two properties that actually matter are
//! observable anywhere: the process exits cleanly on the signal, and the clear runs on the way out.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SET_MARKER: &str = "presence:set";
const CLEAR_MARKER: &str = "presence:clear";

/// Generous enough for a loaded CI runner, short enough to fail rather than hang the suite.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

fn write_config(dir: &std::path::Path) -> std::path::PathBuf {
    // A 1s poll keeps the test quick. The agent rule matches nothing in particular; the loop
    // publishes an idle payload either way, which is all this test needs.
    let body = r#"
poll_interval_secs = 1
min_push_interval_secs = 1

[discord]
client_id = "123456789012345678"

[[agents]]
id = "claude_code"
label = "Claude Code"
process_names = ["claude"]
"#;
    let path = dir.join("config.toml");
    std::fs::write(&path, body).expect("write config");
    path
}

fn spawn_watch(config: &std::path::Path) -> (Child, mpsc::Receiver<String>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devsignal"))
        .args(["watch", "--config"])
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn devsignal watch");

    // Drain stdout on a thread: reading inline would block the test, and letting the pipe fill would
    // block the child.
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
    (child, rx)
}

/// Block until a line with `prefix` arrives, or the timeout expires.
fn wait_for_line(rx: &mpsc::Receiver<String>, prefix: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        match rx.recv_timeout(remaining) {
            Ok(line) if line.starts_with(prefix) => return Some(line),
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

fn signal(child: &Child, sig: &str) {
    let status = Command::new("kill")
        .args([sig, &child.id().to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill {sig} failed");
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => return Some(status),
            None if Instant::now() >= deadline => return None,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// The core assertion, run for each signal: the loop starts, the signal is trapped, the process exits
/// cleanly, and presence is cleared on the way out.
fn assert_clean_shutdown_on(sig: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = write_config(dir.path());
    let (mut child, rx) = spawn_watch(&config);

    // Wait until the loop has actually published once, so we are signalling a running loop rather
    // than racing process startup.
    if wait_for_line(&rx, SET_MARKER, STARTUP_TIMEOUT).is_none() {
        let _ = child.kill();
        panic!("watch never published a `{SET_MARKER}` line within {STARTUP_TIMEOUT:?}");
    }

    signal(&child, sig);

    let status = match wait_for_exit(&mut child, EXIT_TIMEOUT) {
        Some(status) => status,
        None => {
            let _ = child.kill();
            panic!(
                "process ignored {sig} and was still running after {EXIT_TIMEOUT:?} — \
                 is the ctrlc `termination` feature enabled?"
            );
        }
    };

    assert!(
        status.success(),
        "expected a clean exit after {sig}, got {status:?}"
    );

    // The user-visible guarantee: presence does not stay stuck.
    assert!(
        wait_for_line(&rx, CLEAR_MARKER, EXIT_TIMEOUT).is_some(),
        "no `{CLEAR_MARKER}` after {sig}: presence would have been left stale in Discord"
    );
}

/// SIGTERM is what launchd sends (`launchctl bootout`, `kickstart -k`). This is the case that was
/// broken: without the `termination` feature the default disposition terminates the process, so the
/// clear never ran.
#[test]
fn sigterm_exits_cleanly_and_clears_presence() {
    assert_clean_shutdown_on("-TERM");
}

/// SIGINT is Ctrl+C in the foreground. This worked before, and must keep working.
#[test]
fn sigint_exits_cleanly_and_clears_presence() {
    assert_clean_shutdown_on("-INT");
}

/// Shutdown must not wait out the poll interval. `std::thread::sleep` is not interrupted by signal
/// delivery, so the loop sleeps in slices and re-checks the flag; without that, a config with a long
/// `poll_interval_secs` would take that long to stop.
#[test]
fn shutdown_does_not_wait_for_a_long_poll_interval() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("slow.toml");
    std::fs::write(
        &config,
        r#"
poll_interval_secs = 30
min_push_interval_secs = 1

[discord]
client_id = "123456789012345678"

[[agents]]
id = "claude_code"
process_names = ["claude"]
"#,
    )
    .expect("write config");

    let (mut child, rx) = spawn_watch(&config);
    if wait_for_line(&rx, SET_MARKER, STARTUP_TIMEOUT).is_none() {
        let _ = child.kill();
        panic!("watch never published within {STARTUP_TIMEOUT:?}");
    }

    let sent = Instant::now();
    signal(&child, "-TERM");
    let status = match wait_for_exit(&mut child, EXIT_TIMEOUT) {
        Some(status) => status,
        None => {
            let _ = child.kill();
            panic!("process still running {EXIT_TIMEOUT:?} after SIGTERM with a 30s poll interval");
        }
    };
    let elapsed = sent.elapsed();

    assert!(status.success(), "expected a clean exit, got {status:?}");
    assert!(
        elapsed < Duration::from_secs(5),
        "took {elapsed:?} to shut down with poll_interval_secs = 30; \
         the sleep is not being interrupted"
    );
}
