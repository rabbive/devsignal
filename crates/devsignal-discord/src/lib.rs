//! Thin wrapper around `discord-rich-presence` for stable `devsignal` types.

use anyhow::{Context, Result};
use devsignal_core::PresenceView;
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use tracing::warn;

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

        let mut act = activity::Activity::new()
            .details(view.details.clone())
            .state(view.state.clone())
            .assets(assets);

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

/// Apply presence; on IPC failure, try one reconnect.
pub fn set_presence_resilient(session: &mut PresenceSession, view: &PresenceView) {
    if let Err(e) = session.set_presence(view) {
        warn!(error = %e, "presence update failed; reconnecting once");
        if let Err(e2) = session.reconnect() {
            warn!(error = %e2, "reconnect failed");
            return;
        }
        if let Err(e3) = session.set_presence(view) {
            warn!(error = %e3, "presence update failed after reconnect");
        }
    }
}

/// Clear presence; on IPC failure, try one reconnect.
pub fn clear_presence_resilient(session: &mut PresenceSession) {
    if let Err(e) = session.clear() {
        warn!(error = %e, "clear activity failed; reconnecting once");
        if let Err(e2) = session.reconnect() {
            warn!(error = %e2, "reconnect failed");
            return;
        }
        if let Err(e3) = session.clear() {
            warn!(error = %e3, "clear activity failed after reconnect");
        }
    }
}
