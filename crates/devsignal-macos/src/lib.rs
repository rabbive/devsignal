//! macOS frontmost-application detection for labeling IDE / terminal hosts.
//!
//! Uses AppleScript via `/usr/bin/osascript` so this works from a headless daemon.
//! macOS may prompt for **Automation** permission for `System Events` the first time.

#[cfg(target_os = "macos")]
pub fn frontmost_bundle_id() -> Option<String> {
    use std::process::Command;

    const SCRIPT: &str = r#"tell application "System Events" to get bundle identifier of first application process whose frontmost is true"#;

    let output = Command::new("/usr/bin/osascript")
        .args(["-e", SCRIPT])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn frontmost_bundle_id() -> Option<String> {
    None
}
