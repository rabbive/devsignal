//! Where computed presence goes.
//!
//! `devsignal run` sends to Discord; `devsignal watch` prints. The abstraction exists so the poll
//! loop — including its shutdown-and-clear path — can be exercised without a Discord desktop client.
//! That is what makes the SIGTERM guarantee testable in CI instead of taken on trust.

use anyhow::Result;
use devsignal_core::PresenceView;
use devsignal_discord::{clear_presence_resilient, set_presence_resilient, PresenceSession};

/// Line prefixes written by [`StdoutSink`]. `tests/shutdown.rs` matches on these, so treat them as
/// part of the contract rather than cosmetic logging.
pub const SET_MARKER: &str = "presence:set";
pub const CLEAR_MARKER: &str = "presence:clear";

/// Both methods return `Result` because the caller must be able to tell a delivered payload from a
/// dropped one: the debouncer records a send only once the sink confirms it, and a sink that reported
/// nothing used to leave the daemon deduplicating against a payload Discord never received.
pub trait PresenceSink {
    fn set(&mut self, view: &PresenceView) -> Result<()>;
    fn clear(&mut self) -> Result<()>;
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
    fn set(&mut self, view: &PresenceView) -> Result<()> {
        set_presence_resilient(&mut self.session, view)
    }

    fn clear(&mut self) -> Result<()> {
        clear_presence_resilient(&mut self.session)
    }
}

/// Prints what `run` would have published. Backs `devsignal watch`.
///
/// Rust's stdout is a `LineWriter`, so each line is flushed as it is written — a reader on the other
/// end of a pipe sees output tick by tick rather than at exit.
pub struct StdoutSink;

impl PresenceSink for StdoutSink {
    fn set(&mut self, view: &PresenceView) -> Result<()> {
        match serde_json::to_string(view) {
            Ok(json) => println!("{SET_MARKER} {json}"),
            // Serializing a PresenceView cannot realistically fail, but swallowing the line would
            // make `watch` look idle rather than broken.
            Err(e) => println!("{SET_MARKER} <unserializable: {e}>"),
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        println!("{CLEAR_MARKER}");
        Ok(())
    }
}

/// A sink driven by a script of outcomes, for exercising the loop's failure paths.
///
/// Lives here rather than in `main.rs`'s test module so both can use it. `Box<dyn PresenceSink>` has
/// no `Send` bound, so an `Rc<RefCell<_>>` the test keeps a clone of is enough to observe the calls.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct SinkCalls {
    pub sets: usize,
    pub clears: usize,
    /// The view passed to each successful `set`, so a test can assert *what* was published.
    pub sent_views: Vec<PresenceView>,
}

#[cfg(test)]
pub struct ScriptedSink {
    /// One entry per call, in order; `false` fails that call.
    outcomes: Vec<bool>,
    calls: usize,
    log: std::rc::Rc<std::cell::RefCell<SinkCalls>>,
}

#[cfg(test)]
type SinkLog = std::rc::Rc<std::cell::RefCell<SinkCalls>>;

#[cfg(test)]
impl ScriptedSink {
    /// The last scripted entry repeats forever, so `&[false]` is "always fail" and
    /// `&[false, false, true]` is "fail twice, then work".
    pub fn new(outcomes: &[bool]) -> (Self, SinkLog) {
        let log: SinkLog = std::rc::Rc::new(std::cell::RefCell::new(SinkCalls::default()));
        let sink = Self {
            outcomes: outcomes.to_vec(),
            calls: 0,
            log: std::rc::Rc::clone(&log),
        };
        (sink, log)
    }

    fn next_outcome(&mut self) -> Result<()> {
        let ok = self
            .outcomes
            .get(self.calls)
            .or_else(|| self.outcomes.last())
            .copied()
            .unwrap_or(true);
        self.calls += 1;
        if ok {
            Ok(())
        } else {
            Err(anyhow::anyhow!("scripted sink failure #{}", self.calls))
        }
    }
}

#[cfg(test)]
impl PresenceSink for ScriptedSink {
    fn set(&mut self, view: &PresenceView) -> Result<()> {
        self.log.borrow_mut().sets += 1;
        let outcome = self.next_outcome();
        if outcome.is_ok() {
            self.log.borrow_mut().sent_views.push(view.clone());
        }
        outcome
    }

    fn clear(&mut self) -> Result<()> {
        self.log.borrow_mut().clears += 1;
        self.next_outcome()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_view() -> PresenceView {
        PresenceView {
            name: Some("Claude Code".into()),
            details: Some("In VS Code".into()),
            state: Some("devsignal".into()),
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
