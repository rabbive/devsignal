//! Agent detection over time: an agent must be noticed when it starts **and when it stops**.
//!
//! Regression test for a long-standing bug. `System::refresh_specifics` internally passes
//! `remove_dead_processes: false`, so exited processes stayed in the snapshot for the daemon's
//! lifetime. Once an agent CLI had been seen it kept matching forever — presence showed an agent you
//! had already quit, `idle_mode = "clear"` never fired, and the elapsed timer never reset. The bug is
//! invisible to unit tests because it lives in how the process table is refreshed.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SET_MARKER: &str = "presence:set";
/// Generous: each step needs at least one poll tick, and CI runners are slow.
const STEP_TIMEOUT: Duration = Duration::from_secs(25);

/// A process whose name devsignal will match. Copying `sleep` under a chosen filename means both the
/// process name and `argv[0]` basename match, without relying on shell-specific `exec -a`.
fn fake_agent_binary(dir: &std::path::Path) -> std::path::PathBuf {
    let sleep = ["/bin/sleep", "/usr/bin/sleep"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists())
        .expect("a sleep binary must exist");
    let dst = dir.join("devsignal-test-agent");
    std::fs::copy(&sleep, &dst).expect("copy sleep");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dst).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dst, perms).expect("chmod");
    }
    dst
}

fn write_config(dir: &std::path::Path) -> std::path::PathBuf {
    let body = r#"
poll_interval_secs = 1
min_push_interval_secs = 1

[discord]
client_id = "123456789012345678"

[[agents]]
id = "test_agent"
label = "Test Agent"
process_names = ["devsignal-test-agent"]
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

/// Wait for a `presence:set` line whose JSON contains `needle`.
fn wait_for_details(rx: &mpsc::Receiver<String>, needle: &str, timeout: Duration) -> bool {
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
fn an_agent_is_detected_when_it_starts_and_released_when_it_exits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = write_config(dir.path());
    let agent_bin = fake_agent_binary(dir.path());

    let (mut watch, rx) = spawn_watch(&config);

    // 1. Nothing running yet: the payload should be idle.
    assert!(
        wait_for_details(&rx, "\"Idle\"", STEP_TIMEOUT),
        "expected an idle payload before the agent starts"
    );

    // 2. Start the agent; devsignal should pick it up.
    let mut agent = Command::new(&agent_bin)
        .arg("600")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fake agent");

    let detected = wait_for_details(&rx, "Test Agent", STEP_TIMEOUT);
    if !detected {
        let _ = agent.kill();
        let _ = watch.kill();
        panic!("agent was never detected after starting");
    }

    // 3. Stop the agent. This is the assertion that used to fail: the exited process stayed in the
    //    snapshot, so devsignal kept reporting it as running indefinitely.
    let _ = agent.kill();
    let _ = agent.wait();

    let released = wait_for_details(&rx, "\"Idle\"", STEP_TIMEOUT);
    let _ = watch.kill();
    let _ = watch.wait();

    assert!(
        released,
        "devsignal kept reporting the agent after it exited — exited processes are not being \
         removed from the sysinfo snapshot"
    );
}
