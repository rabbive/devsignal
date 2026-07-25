//! Where computed presence goes.
//!
//! `devsignal run` sends to Discord; `devsignal watch` prints. The abstraction exists so the poll
//! loop — including its shutdown-and-clear path — can be exercised without a Discord desktop client.
//! That is what makes the SIGTERM guarantee testable in CI instead of taken on trust.

use devsignal_core::PresenceView;
use devsignal_discord::{clear_presence_resilient, set_presence_resilient, PresenceSession};

/// Line prefixes written by [`StdoutSink`]. `tests/shutdown.rs` matches on these, so treat them as
/// part of the contract rather than cosmetic logging.
pub const SET_MARKER: &str = "presence:set";
pub const CLEAR_MARKER: &str = "presence:clear";

pub trait PresenceSink {
    fn set(&mut self, view: &PresenceView);
    fn clear(&mut self);
}

/// The real thing: Discord over Unix-socket IPC.
pub struct DiscordSink {
    session: PresenceSession,
}

impl DiscordSink {
    pub fn new(session: PresenceSession) -> Self {
        Self { session }
    }
}

impl PresenceSink for DiscordSink {
    fn set(&mut self, view: &PresenceView) {
        set_presence_resilient(&mut self.session, view);
    }

    fn clear(&mut self) {
        clear_presence_resilient(&mut self.session);
    }
}

/// Prints what `run` would have published. Backs `devsignal watch`.
///
/// Rust's stdout is a `LineWriter`, so each line is flushed as it is written — a reader on the other
/// end of a pipe sees output tick by tick rather than at exit.
pub struct StdoutSink;

impl PresenceSink for StdoutSink {
    fn set(&mut self, view: &PresenceView) {
        match serde_json::to_string(view) {
            Ok(json) => println!("{SET_MARKER} {json}"),
            // Serializing a PresenceView cannot realistically fail, but swallowing the line would
            // make `watch` look idle rather than broken.
            Err(e) => println!("{SET_MARKER} <unserializable: {e}>"),
        }
    }

    fn clear(&mut self) {
        println!("{CLEAR_MARKER}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_view() -> PresenceView {
        PresenceView {
            details: "Claude Code".into(),
            state: "In VS Code".into(),
            large_image: "devsignal".into(),
            large_text: String::new(),
            small_image: None,
            small_text: None,
            buttons: vec![],
            start_timestamp_unix: None,
        }
    }

    /// The markers are a contract with the integration test; a rename must break here too.
    #[test]
    fn markers_are_stable_and_distinct() {
        assert_eq!(SET_MARKER, "presence:set");
        assert_eq!(CLEAR_MARKER, "presence:clear");
        assert!(!SET_MARKER.starts_with(CLEAR_MARKER));
        assert!(!CLEAR_MARKER.starts_with(SET_MARKER));
    }

    /// `watch` output must stay one line per action, or a line-oriented reader desynchronises.
    #[test]
    fn serialized_view_is_single_line() {
        let json = serde_json::to_string(&sample_view()).expect("serialize");
        assert!(!json.contains('\n'), "compact JSON must not wrap: {json}");
        assert!(json.contains("Claude Code"));
    }
}
