//! macOS frontmost-application detection for labeling IDE / terminal hosts.
//!
//! Prefers `NSWorkspace` + `NSRunningApplication` (no AppleScript per poll). Falls back to
//! AppleScript (`osascript`) if the native path returns `None` twice in a row.

#[cfg(target_os = "macos")]
mod imp {
    use objc2::rc::autoreleasepool;
    use objc2::rc::DefaultRetained;
    use objc2_app_kit::NSWorkspace;
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NATIVE_MISS_STREAK: AtomicU32 = AtomicU32::new(0);

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
    pub fn frontmost_bundle_id_via_osascript() -> Option<String> {
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

    /// Bundle id of the frontmost app, preferring native AppKit; uses AppleScript after two
    /// consecutive native misses.
    pub fn frontmost_bundle_id() -> Option<String> {
        if let Some(id) = frontmost_bundle_id_native() {
            NATIVE_MISS_STREAK.store(0, Ordering::Relaxed);
            return Some(id);
        }
        let n = NATIVE_MISS_STREAK.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= 2 {
            NATIVE_MISS_STREAK.store(0, Ordering::Relaxed);
            return frontmost_bundle_id_via_osascript();
        }
        None
    }
}

#[cfg(target_os = "macos")]
pub use imp::*;

#[cfg(not(target_os = "macos"))]
pub fn frontmost_bundle_id() -> Option<String> {
    None
}
