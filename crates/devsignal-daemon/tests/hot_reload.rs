//! Config hot-reload.
//!
//! The daemon used to read its config exactly once, so every `devsignal hosts/agents/rules` edit
//! needed a restart — and the subcommands did not say so, leaving users to wonder why a successful
//! "added rule: x" changed nothing.
//!
//! The property that matters most here is the failure case: a config that no longer loads must be
//! reported and ignored, never allowed to take down a running daemon.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SET_MARKER: &str = "presence:set";
const STEP_TIMEOUT: Duration = Duration::from_secs(25);

const BASE_CONFIG: &str = r#"
poll_interval_secs = 1
min_push_interval_secs = 1

[discord]
client_id = "123456789012345678"

[[agents]]
id = "claude_code"
process_names = ["devsignal-nonexistent-agent"]
"#;

fn spawn_watch(config: &std::path::Path) -> (Child, mpsc::Receiver<String>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devsignal"))
        .args(["watch", "--config"])
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn devsignal watch");
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    (child, rx)
}

fn wait_for_state(rx: &mpsc::Receiver<String>, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        match rx.recv_timeout(remaining) {
            Ok(line) if line.starts_with(SET_MARKER) && line.contains(needle) => return true,
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
}

#[test]
fn a_valid_edit_is_picked_up_without_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("config.toml");
    std::fs::write(&config, BASE_CONFIG).expect("write config");

    let (mut watch, rx) = spawn_watch(&config);
    assert!(
        wait_for_state(&rx, "no agent CLI detected", STEP_TIMEOUT),
        "expected the initial idle payload"
    );

    // Add a catch-all rule that rewrites the state line.
    std::fs::write(
        &config,
        format!(
            "{BASE_CONFIG}\n[[rules]]\nname = \"reloaded\"\nwhen = {{}}\nthen = {{ state = \"HotReloaded\" }}\n"
        ),
    )
    .expect("rewrite config");

    let applied = wait_for_state(&rx, "HotReloaded", STEP_TIMEOUT);
    let _ = watch.kill();
    let _ = watch.wait();
    assert!(applied, "the edited config was never applied");
}

#[test]
fn an_invalid_edit_is_ignored_and_the_daemon_keeps_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("config.toml");
    let good = format!(
        "{BASE_CONFIG}\n[[rules]]\nname = \"first\"\nwhen = {{}}\nthen = {{ state = \"FirstState\" }}\n"
    );
    std::fs::write(&config, &good).expect("write config");

    let (mut watch, rx) = spawn_watch(&config);
    assert!(
        wait_for_state(&rx, "FirstState", STEP_TIMEOUT),
        "expected the initial rule to apply"
    );

    // `time = { start = "9" }` is not HH:MM, so Config::validate rejects the whole file.
    std::fs::write(
        &config,
        format!(
            "{BASE_CONFIG}\n[[rules]]\nname = \"broken\"\n\
             when = {{ time = {{ start = \"9\", end = \"5\" }} }}\nthen = {{ hide_host = true }}\n"
        ),
    )
    .expect("write broken config");

    // Give the daemon several poll intervals to notice and reject it.
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        watch.try_wait().expect("try_wait").is_none(),
        "the daemon exited on an invalid config edit; it must keep the previous one and carry on"
    );

    // Restoring a valid file must still be picked up, proving the daemon is not wedged.
    std::fs::write(
        &config,
        format!(
            "{BASE_CONFIG}\n[[rules]]\nname = \"third\"\nwhen = {{}}\nthen = {{ state = \"ThirdState\" }}\n"
        ),
    )
    .expect("restore config");

    let recovered = wait_for_state(&rx, "ThirdState", STEP_TIMEOUT);
    let _ = watch.kill();
    let _ = watch.wait();
    assert!(
        recovered,
        "after an invalid edit was rejected, a later valid edit was not applied"
    );
}
