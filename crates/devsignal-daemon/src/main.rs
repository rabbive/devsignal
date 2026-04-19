use anyhow::{Context, Result};
use devsignal_core::{
    build_presence_view, redact_cwd_basename, select_active_agent, Config, Debouncer, IdleMode,
};
use devsignal_discord::{clear_presence_resilient, set_presence_resilient, PresenceSession};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tracing::info;

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parse_args() -> PathBuf {
    let mut args = std::env::args().skip(1);
    let mut path = Config::default_path();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" | "-c" => {
                if let Some(p) = args.next() {
                    path = PathBuf::from(p);
                } else {
                    eprintln!("--config requires a path");
                    std::process::exit(2);
                }
            }
            "--help" | "-h" => {
                eprintln!(
                    "devsignal — unified Discord Rich Presence for AI coding CLIs\n\
                     \n\
                     Usage:\n\
                       devsignal [--config path]\n\
                     \n\
                     Default config: {}",
                    Config::default_path().display()
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    path
}

fn process_matches_rule(
    name: &str,
    cmd: &[OsString],
    rule: &devsignal_core::AgentRule,
) -> bool {
    let name_l = name.to_lowercase();
    let hit = rule.process_names.iter().any(|n| n.to_lowercase() == name_l);
    if !hit {
        return false;
    }
    if rule.argv_substrings.is_empty() {
        return true;
    }
    let joined = cmd
        .iter()
        .map(|s| s.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    rule
        .argv_substrings
        .iter()
        .any(|needle| joined.contains(needle))
}

fn collect_matches(sys: &System, cfg: &Config) -> Vec<(devsignal_core::AgentRule, u32)> {
    let mut out = Vec::new();
    for (pid, proc) in sys.processes() {
        let name = proc.name().to_string_lossy();
        let cmd = proc.cmd();
        for rule in &cfg.agents {
            if process_matches_rule(&name, cmd, rule) {
                out.push((rule.clone(), pid.as_u32()));
            }
        }
    }
    out
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("devsignal currently supports macOS only");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    {
        match startup() {
            Ok(state) => run_forever(state),
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        }
    }
}

#[cfg(target_os = "macos")]
struct RunState {
    cfg: Config,
    session: PresenceSession,
    sys: System,
    debouncer: Debouncer,
    last_agent_id: Option<String>,
    session_start_unix: Option<u64>,
    poll: Duration,
    first_tick: bool,
}

#[cfg(target_os = "macos")]
fn startup() -> Result<RunState> {
    let cfg_path = parse_args();
    if !cfg_path.exists() {
        anyhow::bail!(
            "config not found at {}\n\
             Copy config.example.toml to that path and set discord.client_id.",
            cfg_path.display()
        );
    }

    let cfg = Config::load_from_path(&cfg_path).context("load config")?;
    let poll = Duration::from_secs(cfg.poll_interval_secs.max(1));
    let debounce_min = Duration::from_secs(cfg.min_push_interval_secs.max(1));

    let mut session = PresenceSession::new(cfg.discord.client_id.clone());
    session.connect().context("ipc connect")?;

    let sys = System::new();
    let debouncer = Debouncer::new(debounce_min);

    info!(config = %cfg_path.display(), "devsignal running");

    Ok(RunState {
        cfg,
        session,
        sys,
        debouncer,
        last_agent_id: None,
        session_start_unix: None,
        poll,
        first_tick: true,
    })
}

#[cfg(target_os = "macos")]
fn run_forever(mut state: RunState) -> ! {
    loop {
        state.sys.refresh_specifics(
            RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
        );

        let matches = collect_matches(&state.sys, &state.cfg);
        let selected = select_active_agent(matches);

        let agent_id = selected.as_ref().map(|(a, _)| a.id.clone());
        let transition = agent_id != state.last_agent_id;

        let entered_idle_clear = selected.is_none() && state.cfg.idle_mode == IdleMode::Clear;

        if entered_idle_clear {
            if transition || state.first_tick {
                clear_presence_resilient(&mut state.session);
            }
            if transition {
                state.session_start_unix = None;
                state.last_agent_id = agent_id;
            }
            state.first_tick = false;
            std::thread::sleep(state.poll);
            continue;
        }

        if transition {
            state.session_start_unix = selected.as_ref().map(|_| now_unix());
            state.last_agent_id = agent_id;
        }

        let cwd_hint = if state.cfg.show_cwd_basename {
            selected.as_ref().and_then(|(_, pid)| {
                state
                    .sys
                    .process(Pid::from_u32(*pid))
                    .and_then(|p| p.cwd())
                    .and_then(|c| redact_cwd_basename(c))
            })
        } else {
            None
        };

        let bundle = devsignal_macos::frontmost_bundle_id();

        let view = build_presence_view(
            &state.cfg,
            selected.as_ref().map(|(a, _)| a),
            bundle.as_deref(),
            state.session_start_unix,
            cwd_hint.as_deref(),
        );

        let force = transition || state.first_tick;
        if state.debouncer.should_push(&view, force) {
            set_presence_resilient(&mut state.session, &view);
        }

        state.first_tick = false;
        std::thread::sleep(state.poll);
    }
}
