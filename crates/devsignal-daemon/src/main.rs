use anyhow::{Context, Result};
use chrono::Timelike;
use devsignal_core::{
    agent_allowed, apply_rules, build_presence_view, host_allowed, host_label_for_bundle,
    process_matches_rule, redact_cwd_basename, select_active_agent, ActiveAgent, AgentRule, Config,
    Debouncer, IdleMode, PresenceView, RuleContext,
};
use devsignal_discord::PresenceSession;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, UpdateKind};
use tracing::{debug, info, warn};

mod cli;
mod config_edit;
mod config_io;
mod init;
mod sink;

use cli::{Cli, DetectScope, RunArgs};
use sink::{DiscordSink, PresenceSink, StdoutSink};

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

/// Only the process fields devsignal actually reads. `nothing()` still yields pid, parent, name,
/// and start time, so this covers `proc.name()`; `cmd` is needed for argv matching, and `cwd` only
/// when the config asks for the project basename. Notably this stops refreshing `environ` for every
/// process on the machine every poll — the most expensive and most privacy-sensitive field.
fn process_refresh_kind(need_cwd: bool) -> ProcessRefreshKind {
    let kind = ProcessRefreshKind::nothing().with_cmd(UpdateKind::OnlyIfNotSet);
    if need_cwd {
        kind.with_cwd(UpdateKind::OnlyIfNotSet)
    } else {
        kind
    }
}

/// Sleep, but notice a shutdown signal promptly. `std::thread::sleep` is not interrupted by signal
/// delivery, so sleeping the whole poll interval in one call meant Ctrl+C took up to that long to be
/// observed — and `poll_interval_secs` has only a lower bound, so a config with `60` meant a
/// 60-second shutdown.
fn sleep_interruptible(total: Duration) {
    const SLICE: Duration = Duration::from_millis(200);
    let deadline = Instant::now() + total;
    while RUNNING.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        std::thread::sleep(SLICE.min(deadline - now));
    }
}

fn refresh_processes(sys: &mut System, need_cwd: bool) {
    sys.refresh_specifics(RefreshKind::nothing().with_processes(process_refresh_kind(need_cwd)));
}

/// One process that satisfied one agent rule.
struct Candidate {
    rule: AgentRule,
    pid: u32,
    process_name: String,
    argv0: Option<String>,
}

fn collect_matches(sys: &System, cfg: &Config) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (pid, proc) in sys.processes() {
        // On Linux, sysinfo enumerates threads alongside processes, and every thread of an agent CLI
        // inherits its argv[0] — so one running CLI would otherwise match a dozen times. Always
        // `None` on macOS, where threads are not listed, so this is a no-op there.
        if proc.thread_kind().is_some() {
            continue;
        }
        let name = proc.name().to_string_lossy();
        let cmd = proc.cmd();
        for rule in &cfg.agents {
            if agent_allowed(cfg, Some(&rule.id)) && process_matches_rule(&name, cmd, rule) {
                out.push(Candidate {
                    rule: rule.clone(),
                    pid: pid.as_u32(),
                    process_name: name.to_string(),
                    argv0: cmd
                        .first()
                        .map(|a| a.to_string_lossy().to_string())
                        .filter(|s| !s.is_empty()),
                });
            }
        }
    }
    out
}

fn winner_of(candidates: &[Candidate]) -> Option<(ActiveAgent, u32)> {
    select_active_agent(
        candidates
            .iter()
            .map(|c| (c.rule.clone(), c.pid))
            .collect::<Vec<_>>(),
    )
}

fn build_policy_view(
    cfg: &Config,
    agent: Option<&ActiveAgent>,
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

/// Resolve the project basename for the winning PID, when the config asks for it.
fn cwd_basename_for(sys: &System, cfg: &Config, pid: Option<u32>) -> Option<String> {
    if !cfg.show_cwd_basename {
        return None;
    }
    sys.process(Pid::from_u32(pid?))
        .and_then(|p| p.cwd())
        .and_then(redact_cwd_basename)
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

fn missing_config_error(path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "config not found at {}\n\
         Run `devsignal init` for a guided setup, or copy config.example.toml to that path \
         and set discord.client_id.",
        path.display()
    )
}

fn load_config(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Err(missing_config_error(path));
    }
    Config::load_from_path(path).context("load config")
}

fn cmd_validate(config_path: &Path) -> Result<()> {
    let cfg = load_config(config_path)?;
    println!("OK: {}", config_path.display());
    println!("discord.client_id: {}", cfg.discord.client_id);
    println!(
        "idle_mode: {:?}  show_cwd_basename: {}  poll: {}s  min_push: {}s",
        cfg.idle_mode, cfg.show_cwd_basename, cfg.poll_interval_secs, cfg.min_push_interval_secs
    );

    for a in &cfg.agents {
        let enabled = agent_allowed(&cfg, Some(&a.id));
        println!(
            "  [[agents]] id={} label={:?} priority={} {}",
            a.id,
            a.label,
            a.priority,
            if enabled { "" } else { "(disabled)" }
        );
        println!(
            "             process_names={:?} argv_substrings={:?}",
            a.process_names, a.argv_substrings
        );
        println!(
            "             large_image={:?} small_image={:?} small_text={:?}",
            a.large_image, a.small_image, a.small_text
        );
        for b in &a.buttons {
            println!("             button: {:?} -> {}", b.label, b.url);
        }
    }

    if !cfg.platforms.disabled_hosts.is_empty() {
        println!("  disabled_hosts: {:?}", cfg.platforms.disabled_hosts);
    }
    if !cfg.platforms.disabled_agents.is_empty() {
        println!("  disabled_agents: {:?}", cfg.platforms.disabled_agents);
    }
    for r in &cfg.rules {
        println!(
            "  [[rules]] name={} hide_host={} state={:?}",
            r.name, r.then.hide_host, r.then.state
        );
    }
    Ok(())
}

fn cmd_once(config_path: &Path) -> Result<()> {
    let cfg = load_config(config_path)?;
    let mut sys = System::new();
    refresh_processes(&mut sys, cfg.show_cwd_basename);
    let candidates = collect_matches(&sys, &cfg);
    let selected = winner_of(&candidates);
    let bundle = devsignal_macos::frontmost_bundle_id();
    // Resolve the CWD exactly as `run` does, so a rule using `when.project_basenames` behaves the
    // same here as in the daemon.
    let cwd = cwd_basename_for(&sys, &cfg, selected.as_ref().map(|(_, pid)| *pid));
    let view = build_policy_view(
        &cfg,
        selected.as_ref().map(|(a, _)| a),
        bundle.as_deref(),
        None,
        cwd.as_deref(),
        Some(local_minutes_now()),
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&view).context("serialize presence view")?
    );
    Ok(())
}

/// Does this argv[0] look like something a user installed, rather than an OS daemon? Used to keep
/// `detect --unmatched` readable: a Mac has hundreds of processes and almost none are agent CLIs.
/// Deliberately generous — `--all` exists for when this filters out the thing you are looking for.
fn looks_user_installed(argv0: &str) -> bool {
    const MARKERS: &[&str] = &[
        "/bin/",
        "/homebrew/",
        "/.local/",
        "/.cargo/",
        "/.bun/",
        "/.deno/",
        "/.volta/",
        "/node_modules/.bin/",
        "/.npm-global/",
        "/.pyenv/",
        "/.rbenv/",
        "/pipx/",
        "/.nvm/",
    ];
    // Bare names (no path at all) are worth showing: that is how a shim on PATH often appears.
    if !argv0.contains('/') {
        return true;
    }
    MARKERS.iter().any(|m| argv0.contains(m))
}

/// List processes that matched no agent rule — the discovery step for adding an agent whose process
/// name you do not know yet.
fn print_unmatched(sys: &System, cfg: &Config, unfiltered: bool) {
    let matched: Vec<u32> = collect_matches(sys, cfg).iter().map(|c| c.pid).collect();

    let mut rows: Vec<(String, String)> = Vec::new();
    for (pid, proc) in sys.processes() {
        if proc.thread_kind().is_some() || matched.contains(&pid.as_u32()) {
            continue;
        }
        let name = proc.name().to_string_lossy().to_string();
        let argv0 = proc
            .cmd()
            .first()
            .map(|a| a.to_string_lossy().to_string())
            .unwrap_or_default();
        if !unfiltered && !looks_user_installed(&argv0) {
            continue;
        }
        rows.push((name, argv0));
    }
    rows.sort();
    rows.dedup();

    if rows.is_empty() {
        println!("\nno unmatched processes to show.");
        if !unfiltered {
            println!("Try `devsignal detect --all` to list every process.");
        }
        return;
    }

    println!(
        "\n{} unmatched process(es){}:",
        rows.len(),
        if unfiltered {
            ""
        } else {
            " that look user-installed"
        }
    );
    for (name, argv0) in &rows {
        println!(
            "  {:<24} argv0={}",
            name,
            if argv0.is_empty() { "<none>" } else { argv0 }
        );
    }
    println!(
        "\nTo track one of these:\n  \
         devsignal agents add --id <id> --label \"<Label>\" --process-name <name>\n\
         Matching is case-insensitive against the process name or the basename of argv0."
    );
    if !unfiltered {
        println!("Not listed? `devsignal detect --all` skips the user-installed filter.");
    }
}

/// Show every process that matched an agent rule, plus which one wins. This is the tool for
/// answering "why isn't my agent detected?" — `agents list` only shows *configured* agents.
fn cmd_detect(config_path: &Path, scope: DetectScope) -> Result<()> {
    let cfg = load_config(config_path)?;
    let mut sys = System::new();
    refresh_processes(&mut sys, cfg.show_cwd_basename);

    if let DetectScope::Unmatched | DetectScope::All = scope {
        print_unmatched(&sys, &cfg, scope == DetectScope::All);
        return Ok(());
    }

    let bundle = devsignal_macos::frontmost_bundle_id();
    match &bundle {
        Some(id) => println!("frontmost host: {} ({})", id, host_label_for_bundle(id)),
        None => println!("frontmost host: <unknown>"),
    }
    if !host_allowed(&cfg, bundle.as_deref()) {
        println!("  note: this host is in platforms.disabled_hosts, so it will be hidden");
    }

    let mut candidates = collect_matches(&sys, &cfg);
    candidates.sort_by(|a, b| {
        a.rule
            .priority
            .cmp(&b.rule.priority)
            .then_with(|| a.pid.cmp(&b.pid))
    });

    if candidates.is_empty() {
        println!("\nno agent processes matched. Rules searched:");
        for a in &cfg.agents {
            if agent_allowed(&cfg, Some(&a.id)) {
                println!(
                    "  {} — process_names={:?} argv_substrings={:?}",
                    a.id, a.process_names, a.argv_substrings
                );
            } else {
                println!("  {} — (disabled via platforms.disabled_agents)", a.id);
            }
        }
        println!(
            "\nCompare against `ps -eo comm,args`. A rule matches on the process name OR the\n\
             basename of argv[0], case-insensitively."
        );
        return Ok(());
    }

    println!("\n{} matching process(es):", candidates.len());
    for c in &candidates {
        println!(
            "  {:<14} pid={:<7} priority={:<5} name={:<18} argv0={}",
            c.rule.id,
            c.pid,
            c.rule.priority,
            c.process_name,
            c.argv0.as_deref().unwrap_or("<none>")
        );
    }

    if let Some((agent, pid)) = winner_of(&candidates) {
        println!(
            "\nwinner: {} (pid {}) — lowest priority wins, ties break on lowest pid",
            agent.id, pid
        );
        let cwd = cwd_basename_for(&sys, &cfg, Some(pid));
        let view = build_policy_view(
            &cfg,
            Some(&agent),
            bundle.as_deref(),
            None,
            cwd.as_deref(),
            Some(local_minutes_now()),
        );
        println!("  details: {}", view.details);
        println!("  state:   {}", view.state);
    }
    Ok(())
}

fn require_macos(what: &str) -> Result<()> {
    anyhow::ensure!(
        cfg!(target_os = "macos"),
        "`devsignal {what}` requires macOS (host detection uses AppKit and autostart uses launchd)"
    );
    Ok(())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        // Logs on stderr, so stdout carries only machine-readable output: `once`'s JSON and
        // `watch`'s presence lines stay pipeable.
        .with_writer(std::io::stderr)
        // Under launchd, stderr is redirected to a log file; colour escapes there are noise.
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match cli::parse_cli(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(2);
        }
    };

    let result = match cli {
        // Explicitly requested help/version go to stdout so they can be piped.
        Cli::Help => {
            print!("{}", cli::global_help());
            Ok(())
        }
        Cli::Version => {
            println!("{}", cli::version_line());
            Ok(())
        }
        Cli::Validate { config } => cmd_validate(&config),
        Cli::Once { config } => cmd_once(&config),
        Cli::Detect { config, scope } => cmd_detect(&config, scope),
        Cli::Watch { config } => cmd_watch(&config),
        Cli::ConfigEdit(cmd) => config_edit::run_config_edit(cmd),
        Cli::Init { config } => require_macos("init").and_then(|()| init::cmd_init(&config)),
        Cli::Run(args) => require_macos("run").and_then(|()| run_daemon(args)),
    };

    if let Err(e) = result {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}

fn install_signal_handler() {
    if let Err(e) = ctrlc::set_handler(|| {
        RUNNING.store(false, Ordering::SeqCst);
    }) {
        // Without a handler we would never clear presence on shutdown, leaving a stale activity in
        // Discord. Surface it rather than swallowing it.
        warn!(error = %e, "could not install signal handler; presence may not clear on exit");
    }
}

fn run_daemon(args: RunArgs) -> Result<()> {
    let cfg = load_config(&args.config)?;
    let mut session = PresenceSession::new(cfg.discord.client_id.clone());
    connect_with_wait(&mut session, args.wait_for_discord).context("ipc connect")?;
    info!(config = %args.config.display(), version = env!("CARGO_PKG_VERSION"), "devsignal running");
    run_loop(cfg, Box::new(DiscordSink::new(session)))
}

/// Same poll loop as `run`, printing instead of talking to Discord. Useful for seeing what the daemon
/// would publish, and it is how the shutdown path is tested without a Discord client.
fn cmd_watch(config_path: &Path) -> Result<()> {
    let cfg = load_config(config_path)?;
    info!(
        config = %config_path.display(),
        "watch mode: computing presence without connecting to Discord (Ctrl+C to stop)"
    );
    run_loop(cfg, Box::new(StdoutSink))
}

fn run_loop(cfg: Config, sink: Box<dyn PresenceSink>) -> Result<()> {
    install_signal_handler();

    let poll = Duration::from_secs(cfg.poll_interval_secs.max(1));
    let debounce_min = Duration::from_secs(cfg.min_push_interval_secs.max(1));
    let debouncer = Debouncer::new(debounce_min);

    let state = RunState {
        cfg,
        sink,
        sys: System::new(),
        debouncer,
        last_agent_id: None,
        session_start_unix: None,
        poll,
        first_tick: true,
    };
    run_forever(state);
    Ok(())
}

struct RunState {
    cfg: Config,
    sink: Box<dyn PresenceSink>,
    sys: System,
    debouncer: Debouncer,
    last_agent_id: Option<String>,
    session_start_unix: Option<u64>,
    poll: Duration,
    first_tick: bool,
}

fn run_forever(mut state: RunState) {
    while RUNNING.load(Ordering::SeqCst) {
        refresh_processes(&mut state.sys, state.cfg.show_cwd_basename);

        let candidates = collect_matches(&state.sys, &state.cfg);
        let selected = winner_of(&candidates);

        let agent_id = selected.as_ref().map(|(a, _)| a.id.clone());
        let transition = agent_id != state.last_agent_id;
        if transition {
            debug!(?agent_id, candidates = candidates.len(), "agent transition");
        }

        let entered_idle_clear = selected.is_none() && state.cfg.idle_mode == IdleMode::Clear;

        if entered_idle_clear {
            if transition || state.first_tick {
                state.sink.clear();
            }
            if transition {
                state.session_start_unix = None;
                state.last_agent_id = agent_id;
            }
            state.first_tick = false;
            sleep_interruptible(state.poll);
            continue;
        }

        if transition {
            state.session_start_unix = selected.as_ref().map(|_| now_unix());
            state.last_agent_id = agent_id;
        }

        let cwd_hint = cwd_basename_for(
            &state.sys,
            &state.cfg,
            selected.as_ref().map(|(_, pid)| *pid),
        );

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
            debug!(details = %view.details, state = %view.state, "pushing presence");
            state.sink.set(&view);
        }

        state.first_tick = false;
        sleep_interruptible(state.poll);
    }

    // The whole point of trapping SIGTERM: never leave a stale activity behind.
    state.sink.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_host_state_covers_active_idle_and_project() {
        assert_eq!(hidden_host_state(true, None), "Working");
        assert_eq!(hidden_host_state(true, Some("myrepo")), "Working · myrepo");
        assert_eq!(hidden_host_state(true, Some("")), "Working");
        assert_eq!(hidden_host_state(false, Some("x")), "No agent CLI detected");
    }

    #[test]
    fn process_refresh_kind_only_asks_for_what_we_read() {
        let lean = process_refresh_kind(false);
        assert_eq!(lean.cwd(), UpdateKind::Never);
        assert_eq!(lean.cmd(), UpdateKind::OnlyIfNotSet);
        // The expensive/sensitive fields must stay off.
        assert_eq!(lean.environ(), UpdateKind::Never);
        assert!(!lean.cpu());
        assert!(!lean.memory());
        assert!(!lean.disk_usage());

        let with_cwd = process_refresh_kind(true);
        assert_eq!(with_cwd.cwd(), UpdateKind::OnlyIfNotSet);
        assert_eq!(with_cwd.environ(), UpdateKind::Never);
    }

    #[test]
    fn user_installed_heuristic_accepts_agent_cli_shapes() {
        // The shapes agent CLIs actually take.
        for argv0 in [
            "/opt/homebrew/bin/gemini",
            "/usr/local/bin/codex",
            "/Users/me/.local/bin/aider",
            "/Users/me/.bun/bin/opencode",
            "/Users/me/.cargo/bin/devsignal",
            "/Users/me/project/node_modules/.bin/cline",
            "claude",
        ] {
            assert!(looks_user_installed(argv0), "{argv0} should be listed");
        }
    }

    #[test]
    fn user_installed_heuristic_filters_system_daemons() {
        for argv0 in [
            "/System/Library/PrivateFrameworks/X.framework/Support/xd",
            "/usr/libexec/secinitd",
            "/Applications/Safari.app/Contents/MacOS/Safari",
        ] {
            assert!(
                !looks_user_installed(argv0),
                "{argv0} should be filtered out"
            );
        }
    }

    #[test]
    fn missing_config_error_points_at_init() {
        let msg = format!("{}", missing_config_error(Path::new("/nope/x.toml")));
        assert!(msg.contains("/nope/x.toml"), "got {msg}");
        assert!(msg.contains("devsignal init"), "got {msg}");
    }
}
