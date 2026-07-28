//! Thin wrapper around `discord-rich-presence` for stable `devsignal` types.

use anyhow::{Context, Result};
use devsignal_core::PresenceView;
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};

/// The three IPC operations the resilient helpers need.
///
/// This exists so the reconnect-once policy is testable without a Discord desktop client:
/// `PresenceSession` wraps a concrete `DiscordIpcClient` that cannot be faked, which is why this
/// crate had no tests at all.
pub trait PresenceIpc {
    fn set_presence(&mut self, view: &PresenceView) -> Result<()>;
    fn clear(&mut self) -> Result<()>;
    fn reconnect(&mut self) -> Result<()>;
    fn connect(&mut self) -> Result<()>;
}

impl PresenceIpc for PresenceSession {
    // Spelled with the explicit path rather than `self.set_presence(view)`: inherent methods shadow
    // trait ones, so the bare call would compile as infinite recursion if the inherent method were
    // ever removed.
    fn set_presence(&mut self, view: &PresenceView) -> Result<()> {
        PresenceSession::set_presence(self, view)
    }

    fn clear(&mut self) -> Result<()> {
        PresenceSession::clear(self)
    }

    fn reconnect(&mut self) -> Result<()> {
        PresenceSession::reconnect(self)
    }

    fn connect(&mut self) -> Result<()> {
        PresenceSession::connect(self)
    }
}

pub struct PresenceSession {
    client: DiscordIpcClient,
}

impl PresenceSession {
    pub fn new(client_id: impl Into<String>) -> Self {
        let client_id = client_id.into();
        let client = DiscordIpcClient::new(&client_id);
        Self { client }
    }

    pub fn connect(&mut self) -> Result<()> {
        self.client
            .connect()
            .map_err(|e| anyhow::anyhow!(e))
            .context("connect to Discord IPC (is Discord running?)")
    }

    pub fn reconnect(&mut self) -> Result<()> {
        self.client
            .reconnect()
            .map_err(|e| anyhow::anyhow!(e))
            .context("reconnect Discord IPC (is Discord running?)")
    }

    pub fn set_presence(&mut self, view: &PresenceView) -> Result<()> {
        let mut assets = activity::Assets::new()
            .large_image(view.large_image.clone())
            .large_text(view.large_text.clone());

        if let Some(ref si) = view.small_image {
            assets = assets.small_image(si.clone());
        }
        if let Some(ref st) = view.small_text {
            assets = assets.small_text(st.clone());
        }

        let mut act = activity::Activity::new().assets(assets);

        // Discord renders name / details / state as three lines, in that order. `name` overrides
        // the Discord application's own name, which is what puts the agent on the first line
        // instead of "devsignal"; leaving any of them unset omits that line rather than showing an
        // empty one.
        if let Some(ref name) = view.name {
            act = act.name(name.clone());
        }
        if let Some(ref details) = view.details {
            act = act.details(details.clone());
        }
        if let Some(ref state) = view.state {
            act = act.state(state.clone());
        }

        if let Some(ts) = view.start_timestamp_unix {
            act = act.timestamps(activity::Timestamps::new().start(ts as i64));
        }

        let btns: Vec<activity::Button> = view
            .buttons
            .iter()
            .take(2)
            .map(|b| activity::Button::new(b.label.as_str(), b.url.as_str()))
            .collect();
        if !btns.is_empty() {
            act = act.buttons(btns);
        }

        self.client
            .set_activity(act)
            .map_err(|e| anyhow::anyhow!(e))
            .context("set Discord activity")
    }

    pub fn clear(&mut self) -> Result<()> {
        self.client
            .clear_activity()
            .map_err(|e| anyhow::anyhow!(e))
            .context("clear Discord activity")
    }
}

/// Re-establish the IPC connection: one `reconnect`, falling back to a plain `connect`.
///
/// The fallback is **not** redundant. `DiscordIpcClient::reconnect` starts with `close()`, and `close()`
/// returns `Err(NotConnected)` when there is no socket — so `reconnect` propagates that and never
/// reaches `connect_ipc`. A client that has never connected therefore cannot be recovered by
/// `reconnect` at all, and that is exactly the daemon's state when Discord was not running at login:
/// `connect_with_wait` timed out, which is deliberately non-fatal, leaving a session with no socket.
/// Without this fallback the daemon would stay alive holding the instance lock and never connect.
///
/// It also removes the dependency on `reconnect` succeeding for a socket that died while the machine
/// slept — if it cannot, a fresh `connect` still gets there.
fn reestablish<T: PresenceIpc>(ipc: &mut T) -> Result<()> {
    match ipc.reconnect() {
        Ok(()) => Ok(()),
        Err(reconnect_err) => ipc.connect().with_context(|| {
            format!("reconnect failed ({reconnect_err:#}), and so did a fresh connect")
        }),
    }
}

/// Apply presence; on IPC failure, re-establish the connection once and retry.
///
/// Returns the failure rather than logging it. The caller needs to know: a swallowed failure used to
/// be recorded as a successful send, after which an unchanged payload was deduped forever and no
/// reconnect was ever attempted again.
pub fn set_presence_resilient<T: PresenceIpc>(ipc: &mut T, view: &PresenceView) -> Result<()> {
    match ipc.set_presence(view) {
        Ok(()) => return Ok(()),
        Err(first) => {
            // A Discord that restarted invalidates the socket with no other symptom, so one
            // reconnect-and-retry is worth trying before reporting failure.
            reestablish(ipc)
                .with_context(|| format!("recover after a failed presence update ({first:#})"))?;
        }
    }
    ipc.set_presence(view)
        .context("set presence after reconnecting")
}

/// Clear presence; on IPC failure, re-establish the connection once and retry.
pub fn clear_presence_resilient<T: PresenceIpc>(ipc: &mut T) -> Result<()> {
    match ipc.clear() {
        Ok(()) => return Ok(()),
        Err(first) => {
            reestablish(ipc)
                .with_context(|| format!("recover after a failed clear ({first:#})"))?;
        }
    }
    ipc.clear().context("clear presence after reconnecting")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An IPC fake driven by a script of outcomes, counting what it was asked to do.
    struct ScriptedIpc {
        /// One entry per `set`/`clear` call, in order; `false` means that call fails. Calls past the
        /// end succeed, so a short script means "fail the first N, then work".
        outcomes: Vec<bool>,
        reconnect_ok: bool,
        /// Whether a plain `connect` succeeds. Defaults to true, since the interesting case is a
        /// never-connected client that `reconnect` cannot fix but `connect` can.
        connect_ok: bool,
        calls: usize,
        reconnects: usize,
        connects: usize,
    }

    impl ScriptedIpc {
        fn new(outcomes: &[bool], reconnect_ok: bool) -> Self {
            Self {
                outcomes: outcomes.to_vec(),
                reconnect_ok,
                connect_ok: true,
                calls: 0,
                reconnects: 0,
                connects: 0,
            }
        }

        fn with_connect_ok(mut self, ok: bool) -> Self {
            self.connect_ok = ok;
            self
        }

        fn next_outcome(&mut self) -> Result<()> {
            let ok = self.outcomes.get(self.calls).copied().unwrap_or(true);
            self.calls += 1;
            if ok {
                Ok(())
            } else {
                Err(anyhow::anyhow!("scripted ipc failure #{}", self.calls))
            }
        }
    }

    impl PresenceIpc for ScriptedIpc {
        fn set_presence(&mut self, _view: &PresenceView) -> Result<()> {
            self.next_outcome()
        }

        fn clear(&mut self) -> Result<()> {
            self.next_outcome()
        }

        fn reconnect(&mut self) -> Result<()> {
            self.reconnects += 1;
            if self.reconnect_ok {
                Ok(())
            } else {
                Err(anyhow::anyhow!("scripted reconnect failure"))
            }
        }

        fn connect(&mut self) -> Result<()> {
            self.connects += 1;
            if self.connect_ok {
                Ok(())
            } else {
                Err(anyhow::anyhow!("scripted connect failure"))
            }
        }
    }

    fn view() -> PresenceView {
        PresenceView {
            name: Some("Claude Code".into()),
            details: Some("In VS Code".into()),
            state: Some("devsignal".into()),
            large_image: "devsignal".into(),
            large_text: "devsignal".into(),
            small_image: None,
            small_text: None,
            buttons: vec![],
            start_timestamp_unix: None,
        }
    }

    #[test]
    fn a_successful_set_does_not_reconnect() {
        let mut ipc = ScriptedIpc::new(&[true], true);
        assert!(set_presence_resilient(&mut ipc, &view()).is_ok());
        assert_eq!(ipc.calls, 1);
        assert_eq!(ipc.reconnects, 0);
    }

    #[test]
    fn a_failed_set_reconnects_once_and_retries() {
        let mut ipc = ScriptedIpc::new(&[false], true);
        assert!(set_presence_resilient(&mut ipc, &view()).is_ok());
        assert_eq!(ipc.calls, 2, "the retry after reconnecting must happen");
        assert_eq!(ipc.reconnects, 1, "exactly one reconnect, not a loop");
    }

    /// The regression this exists for: `DiscordIpcClient::reconnect` begins with `close()`, which
    /// returns `Err(NotConnected)` when there is no socket, so `reconnect` never reaches `connect_ipc`
    /// and **cannot** recover a client that never connected. That is precisely the daemon's state after
    /// a non-fatal startup connect timeout — Discord was not running at login. Without the `connect`
    /// fallback the daemon stays alive holding the instance lock and never publishes anything.
    #[test]
    fn a_never_connected_session_is_recovered_by_a_plain_connect() {
        // reconnect always fails (as it does with no socket); connect works.
        let mut ipc = ScriptedIpc::new(&[false], false);
        assert!(
            set_presence_resilient(&mut ipc, &view()).is_ok(),
            "a never-connected session must still be recoverable"
        );
        assert_eq!(ipc.reconnects, 1, "reconnect is tried first");
        assert_eq!(ipc.connects, 1, "then a plain connect, which is what works");
        assert_eq!(ipc.calls, 2, "and the send is retried after connecting");
    }

    /// The fallback must not fire when it is not needed — a working `reconnect` is the cheaper path and
    /// a redundant `connect` would replace a live socket.
    #[test]
    fn connect_is_not_attempted_when_reconnect_succeeds() {
        let mut ipc = ScriptedIpc::new(&[false], true);
        assert!(set_presence_resilient(&mut ipc, &view()).is_ok());
        assert_eq!(ipc.reconnects, 1);
        assert_eq!(ipc.connects, 0);
    }

    #[test]
    fn a_never_connected_session_is_recovered_on_the_clear_path_too() {
        let mut ipc = ScriptedIpc::new(&[false], false);
        assert!(clear_presence_resilient(&mut ipc).is_ok());
        assert_eq!(ipc.connects, 1);
        assert_eq!(ipc.calls, 2);
    }

    /// The early return when recovery fails outright is a real property: retrying the send over a
    /// socket we know is dead would just produce a second, less informative error.
    #[test]
    fn a_failed_reconnect_returns_err_without_a_second_set() {
        let mut ipc = ScriptedIpc::new(&[false], false).with_connect_ok(false);
        let err = set_presence_resilient(&mut ipc, &view()).expect_err("must report failure");
        assert_eq!(ipc.calls, 1);
        assert_eq!(ipc.reconnects, 1);
        assert_eq!(
            ipc.connects, 1,
            "the fallback is attempted before giving up"
        );
        // The original failure is preserved in the reconnect error's context, so a log line names
        // what actually went wrong first.
        let chain = format!("{err:#}");
        assert!(
            chain.contains("scripted ipc failure #1"),
            "the first failure must survive into the error chain: {chain}"
        );
    }

    #[test]
    fn a_retry_that_also_fails_returns_err() {
        let mut ipc = ScriptedIpc::new(&[false, false], true);
        assert!(set_presence_resilient(&mut ipc, &view()).is_err());
        assert_eq!(ipc.calls, 2);
        assert_eq!(ipc.reconnects, 1);
    }

    #[test]
    fn a_failed_clear_reconnects_once_and_retries() {
        let mut ipc = ScriptedIpc::new(&[false], true);
        assert!(clear_presence_resilient(&mut ipc).is_ok());
        assert_eq!(ipc.calls, 2);
        assert_eq!(ipc.reconnects, 1);
    }
}
