use anyhow::{Context, Result};
use chrono::Timelike;
use devsignal_core::{
    agent_allowed, apply_rules, build_presence_view, host_allowed, process_matches_rule,
    redact_cwd_basename, select_active_agent, Config, Debouncer, IdleMode, PresenceView,
    RuleContext,
};
use devsignal_discord::{clear_presence_resilient, set_presence_resilient, PresenceSession};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tracing::{info, warn};

mod config_edit;
mod init;

static RUNNING: AtomicBool = AtomicBool::new(true);

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn local_minutes_now() -> u16 {
    let now = chrono::Local::now();
    (now.hour() as u16) * 60 + now.minute() as u16
}

fn hidden_host_state(active: bool, cwd_basename: Option<&str>) -> String {
    if active {
        cwd_basename
            .filter(|s| !s.is_empty())
            .map(|s| format!("Working · {s}"))
            .unwrap_or_else(|| "Working".to_string())
    } else {
        "No agent CLI detected".to_string()
    }
}

#[derive(Debug)]
enum Cli {
    Run(RunArgs),
    Validate { config: PathBuf },
    Once { config: PathBuf },
    Init { config: PathBuf },
    ConfigEdit(config_edit::ConfigEditCommand),
}

#[derive(Debug)]
struct RunArgs {
    config: PathBuf,
    /// When true, retry Discord IPC until timeout if Discord is not running.
    wait_for_discord: bool,
}

fn print_global_help() {
    eprintln!(
        "devsignal — unified Discord Rich Presence for AI coding CLIs (macOS)\n\
         \n\
         Usage:\n\
           devsignal [run] [options]\n\
           devsignal init [--config path]\n\
           devsignal validate [--config path]\n\
           devsignal once [--config path]\n\
           devsignal hosts list|enable|disable ...\n\
           devsignal agents list|enable|disable ...\n\
           devsignal rules list|add|remove ...\n\
         \n\
         Default config: {}\n\
         \n\
         Run options:\n\
           -c, --config <path>     Config file (default: see above)\n\
           --wait-for-discord      Retry until Discord is available (default)\n\
           --no-wait-for-discord   Fail immediately if Discord IPC is unavailable\n",
        Config::default_path().display()
    );
}

fn parse_config_path_only(args: &[String]) -> Result<PathBuf> {
    let mut path = Config::default_path();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" | "-c" => {
                let p = it.next().context("--config requires a path")?;
                path = PathBuf::from(p);
            }
            "--help" | "-h" => {
                print_global_help();
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(path)
}

fn parse_run_args(args: &[String]) -> Result<RunArgs> {
    let mut path = Config::default_path();
    let mut wait_for_discord = true;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" | "-c" => {
                let p = it.next().context("--config requires a path")?;
                path = PathBuf::from(p);
            }
            "--wait-for-discord" => wait_for_discord = true,
            "--no-wait-for-discord" => wait_for_discord = false,
            "--help" | "-h" => {
                print_global_help();
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(RunArgs {
        config: path,
        wait_for_discord,
    })
}

fn parse_cli() -> Result<Cli> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Ok(Cli::Run(RunArgs {
            config: Config::default_path(),
            wait_for_discord: true,
        }));
    }
    match args[0].as_str() {
        "init" => {
            let rest = &args[1..];
            Ok(Cli::Init {
                config: parse_config_path_only(rest)?,
            })
        }
        "validate" => {
            let rest = &args[1..];
            Ok(Cli::Validate {
                config: parse_config_path_only(rest)?,
            })
        }
        "once" => {
            let rest = &args[1..];
            Ok(Cli::Once {
                config: parse_config_path_only(rest)?,
            })
        }
        "run" => {
            let rest = &args[1..];
            Ok(Cli::Run(parse_run_args(rest)?))
        }
        "hosts" => {
            let rest = &args[1..];
            Ok(Cli::ConfigEdit(config_edit::parse_hosts_command(rest)?))
        }
        "agents" => {
            let rest = &args[1..];
            Ok(Cli::ConfigEdit(config_edit::parse_agents_command(rest)?))
        }
        "rules" => {
            let rest = &args[1..];
            Ok(Cli::ConfigEdit(config_edit::parse_rules_command(rest)?))
        }
        "help" | "--help" | "-h" => {
            print_global_help();
            std::process::exit(0);
        }
        // Legacy: `devsignal --config foo` without subcommand
        _ => Ok(Cli::Run(parse_run_args(&args)?)),
    }
}

fn collect_matches(sys: &System, cfg: &Config) -> Vec<(devsignal_core::AgentRule, u32)> {
    let mut out = Vec::new();
    for (pid, proc) in sys.processes() {
        let name = proc.name().to_string_lossy();
        let cmd = proc.cmd();
        for rule in &cfg.agents {
            if agent_allowed(cfg, Some(&rule.id)) && process_matches_rule(&name, cmd, rule) {
                out.push((rule.clone(), pid.as_u32()));
            }
        }
    }
    out
}

fn build_policy_view(
    cfg: &Config,
    agent: Option<&devsignal_core::ActiveAgent>,
    host_bundle_id: Option<&str>,
    session_start_unix: Option<u64>,
    cwd_basename: Option<&str>,
    local_minutes: Option<u16>,
) -> PresenceView {
    let host_is_allowed = host_allowed(cfg, host_bundle_id);
    let ctx = RuleContext {
        host_bundle_id,
        agent_id: agent.map(|a| a.id.as_str()),
        cwd_basename,
        active: agent.is_some(),
        local_minutes,
    };
    let policy = apply_rules(cfg, &ctx);
    let hide_host = !host_is_allowed || policy.hide_host;
    let visible_host = if hide_host { None } else { host_bundle_id };

    let mut view = build_presence_view(cfg, agent, visible_host, session_start_unix, cwd_basename);
    if hide_host && policy.state.is_none() {
        view.state = hidden_host_state(agent.is_some(), cwd_basename);
    }
    if let Some(state) = policy.state {
        view.state = state;
    }
    view
}

fn connect_with_wait(session: &mut PresenceSession, wait: bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut sleep_dur = Duration::from_millis(400);
    loop {
        match session.connect() {
            Ok(()) => return Ok(()),
            Err(e) => {
                if !wait || Instant::now() >= deadline {
                    return Err(e).context("connect to Discord IPC (is Discord running?)");
                }
                warn!(error = %e, "Discord not reachable; retrying IPC");
                std::thread::sleep(sleep_dur);
                sleep_dur = (sleep_dur * 2).min(Duration::from_secs(4));
            }
        }
    }
}

fn cmd_validate(config_path: &Path) -> Result<()> {
    if !config_path.exists() {
        anyhow::bail!("config not found at {}", config_path.display());
    }
    let cfg = Config::load_from_path(config_path).context("load config")?;
    println!("OK: {}", config_path.display());
    println!("discord.client_id: {}", cfg.discord.client_id);
    for a in &cfg.agents {
        println!(
            "  [[agents]] id={} label={:?} priority={} process_names={:?} argv_substrings={:?}",
            a.id, a.label, a.priority, a.process_names, a.argv_substrings
        );
    }
    Ok(())
}

fn cmd_once(config_path: &Path) -> Result<()> {
    if !config_path.exists() {
        anyhow::bail!("config not found at {}", config_path.display());
    }
    let cfg = Config::load_from_path(config_path).context("load config")?;
    let mut sys = System::new();
    sys.refresh_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()));
    let matches = collect_matches(&sys, &cfg);
    let selected = select_active_agent(matches);
    let bundle = devsignal_macos::frontmost_bundle_id();
    let view = build_policy_view(
        &cfg,
        selected.as_ref().map(|(a, _)| a),
        bundle.as_deref(),
        None,
        None,
        Some(local_minutes_now()),
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&view).context("serialize presence view")?
    );
    Ok(())
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
        let cli = match parse_cli() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(2);
            }
        };

        let code = match cli {
            Cli::Init { config } => match init::cmd_init(&config) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e:#}");
                    1
                }
            },
            Cli::Validate { config } => match cmd_validate(&config) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e:#}");
                    1
                }
            },
            Cli::Once { config } => match cmd_once(&config) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e:#}");
                    1
                }
            },
            Cli::Run(args) => match run_daemon(args) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e:#}");
                    1
                }
            },
            Cli::ConfigEdit(cmd) => match config_edit::run_config_edit(cmd) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e:#}");
                    1
                }
            },
        };
        std::process::exit(code);
    }
}

#[cfg(target_os = "macos")]
fn run_daemon(args: RunArgs) -> Result<()> {
    let _ = ctrlc::set_handler(|| {
        RUNNING.store(false, Ordering::SeqCst);
    });

    if !args.config.exists() {
        anyhow::bail!(
            "config not found at {}\n\
             Copy config.example.toml to that path and set discord.client_id.",
            args.config.display()
        );
    }

    let cfg = Config::load_from_path(&args.config).context("load config")?;
    let poll = Duration::from_secs(cfg.poll_interval_secs.max(1));
    let debounce_min = Duration::from_secs(cfg.min_push_interval_secs.max(1));

    let mut session = PresenceSession::new(cfg.discord.client_id.clone());
    connect_with_wait(&mut session, args.wait_for_discord).context("ipc connect")?;

    let sys = System::new();
    let debouncer = Debouncer::new(debounce_min);

    info!(config = %args.config.display(), "devsignal running");

    let state = RunState {
        cfg,
        session,
        sys,
        debouncer,
        last_agent_id: None,
        session_start_unix: None,
        poll,
        first_tick: true,
    };
    run_forever(state);
    Ok(())
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
fn run_forever(mut state: RunState) {
    while RUNNING.load(Ordering::SeqCst) {
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
                    .and_then(redact_cwd_basename)
            })
        } else {
            None
        };

        let bundle = devsignal_macos::frontmost_bundle_id();

        let view = build_policy_view(
            &state.cfg,
            selected.as_ref().map(|(a, _)| a),
            bundle.as_deref(),
            state.session_start_unix,
            cwd_hint.as_deref(),
            Some(local_minutes_now()),
        );

        let force = transition || state.first_tick;
        if state.debouncer.should_push(&view, force) {
            set_presence_resilient(&mut state.session, &view);
        }

        state.first_tick = false;
        std::thread::sleep(state.poll);
    }

    clear_presence_resilient(&mut state.session);
}
