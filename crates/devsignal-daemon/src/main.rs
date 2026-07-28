use anyhow::{Context, Result};
use chrono::Timelike;
use devsignal_core::{
    agent_allowed, apply_rules, build_presence_view, host_allowed, host_label_for_bundle,
    process_matches_rule, redact_cwd_basename, select_active_agent, ActiveAgent, AgentRule, Config,
    Debouncer, IdleMode, ImageMode, PresenceAction, PresenceInputs, PresencePolicyOverride,
    PresenceView, RetryBackoff, RuleContext,
};
use devsignal_discord::PresenceSession;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tracing::{debug, info, warn};

mod cli;
mod config_edit;
mod config_io;
mod init;
mod lockfile;
mod sink;

use cli::{Cli, DetectScope, RunArgs};
use sink::{DiscordSink, PresenceSink, StdoutSink};

static RUNNING: AtomicBool = AtomicBool::new(true);

/// Stand-ins used by `detect` for the two ways a line can be absent: `presence.name = "off"` hands
/// line 1 back to Discord, while any other slot set to `off` drops its line entirely.
const APP_NAME_PLACEHOLDER: &str = "<Discord application name>";
const OMITTED: &str = "<omitted>";

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

/// Refresh the process table, **removing processes that have exited**.
///
/// `System::refresh_specifics` internally passes `remove_dead_processes: false`, so exited processes
/// stayed in the snapshot forever. Once an agent CLI had been seen it matched for the rest of the
/// daemon's life: presence kept showing an agent you had quit, `idle_mode = "clear"` never fired, and
/// the session timer never reset. Call the explicit form so the second argument is visible.
fn refresh_processes(sys: &mut System, need_cwd: bool) {
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh_kind(need_cwd));
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

/// Build the payload, and report which `[[rules]]` entry (if any) shaped it.
///
/// The rule name is returned separately rather than added to `PresenceView`, because that struct is
/// the debouncer's equality key: including it would trigger a Discord write whenever the matched rule
/// changed, even with identical visible text.
fn build_policy_view(
    cfg: &Config,
    agent: Option<&ActiveAgent>,
    host_bundle_id: Option<&str>,
    session_start_unix: Option<u64>,
    cwd_basename: Option<&str>,
    local_minutes: Option<u16>,
) -> (PresenceView, PresencePolicyOverride) {
    let host_is_allowed = host_allowed(cfg, host_bundle_id);
    let ctx = RuleContext {
        host_bundle_id,
        agent_id: agent.map(|a| a.id.as_str()),
        cwd_basename,
        active: agent.is_some(),
        local_minutes,
    };
    let policy = apply_rules(cfg, &ctx);

    let mut view = build_presence_view(
        cfg,
        &PresenceInputs {
            agent,
            host_bundle_id,
            hide_host: !host_is_allowed || policy.hide_host,
            session_start_unix,
            cwd_basename,
        },
    );
    if let Some(state) = policy.state.clone() {
        view.state = Some(state);
    }
    (view, policy)
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
    // The card, top line first, so a layout mistake is visible without starting the daemon.
    println!(
        "lines: name={:?} details={:?} state={:?}  (name=off means Discord shows the app name)",
        cfg.presence.name, cfg.presence.details, cfg.presence.state
    );
    println!(
        "images: mode={:?} host_icon={}{}",
        cfg.images.mode,
        cfg.images.host_icon,
        if cfg.images.mode == ImageMode::Url {
            format!("  base_url={}", cfg.images.base_url)
        } else {
            String::new()
        }
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
    let (view, policy) = build_policy_view(
        &cfg,
        selected.as_ref().map(|(a, _)| a),
        bundle.as_deref(),
        None,
        cwd.as_deref(),
        Some(local_minutes_now()),
    );

    // `matched_rule` is reported here but is not part of PresenceView: see build_policy_view.
    #[derive(serde::Serialize)]
    struct OnceOutput<'a> {
        #[serde(flatten)]
        view: &'a PresenceView,
        matched_rule: Option<&'a str>,
    }
    let out = OnceOutput {
        view: &view,
        matched_rule: policy.matched_rule_name.as_deref(),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&out).context("serialize presence view")?
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
        let (view, policy) = build_policy_view(
            &cfg,
            Some(&agent),
            bundle.as_deref(),
            None,
            cwd.as_deref(),
            Some(local_minutes_now()),
        );
        // In card order: name is line 1, details line 2, state line 3.
        println!(
            "  name:    {}",
            view.name.as_deref().unwrap_or(APP_NAME_PLACEHOLDER)
        );
        println!("  details: {}", view.details.as_deref().unwrap_or(OMITTED));
        println!("  state:   {}", view.state.as_deref().unwrap_or(OMITTED));
        println!("  large_image: {}", view.large_image);
        println!(
            "  small_image: {}",
            view.small_image.as_deref().unwrap_or(OMITTED)
        );
        match policy.matched_rule_name.as_deref() {
            Some(name) => println!("  matched rule: {name}"),
            None if cfg.rules.is_empty() => {
                println!("  matched rule: none (no [[rules]] configured)")
            }
            None => println!(
                "  matched rule: none of the {} configured rules",
                cfg.rules.len()
            ),
        }
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

    // Only `run` takes the lock. `watch` never touches Discord, so locking it out would break the
    // legitimate "watch what the daemon is computing while it runs" workflow. Held for the lifetime of
    // the loop — a bare `let _` would drop it here and disable the check.
    let _lock = lockfile::acquire(&args.config)?;

    let mut session = PresenceSession::new(cfg.discord.client_id.clone());
    if let Err(e) = connect_with_wait(&mut session, args.wait_for_discord) {
        // At login launchd starts devsignal before Discord has finished launching, so this is the
        // common case, not an edge case. Failing here meant exit 1, and `KeepAlive` respawning the
        // daemon every 10 seconds forever. The poll loop retries with backoff, so enter it anyway.
        // `--no-wait-for-discord` keeps its fail-fast contract for scripts.
        if !args.wait_for_discord {
            return Err(e).context("ipc connect");
        }
        warn!(
            error = %format!("{e:#}"),
            "Discord not reachable at startup; the poll loop will keep retrying"
        );
    }
    info!(config = %args.config.display(), version = env!("CARGO_PKG_VERSION"), "devsignal running");
    run_loop(args.config, cfg, Box::new(DiscordSink::new(session)))
}

/// Same poll loop as `run`, printing instead of talking to Discord. Useful for seeing what the daemon
/// would publish, and it is how the shutdown path is tested without a Discord client.
fn cmd_watch(config_path: &Path) -> Result<()> {
    let cfg = load_config(config_path)?;
    info!(
        config = %config_path.display(),
        "watch mode: computing presence without connecting to Discord (Ctrl+C to stop)"
    );
    run_loop(config_path.to_path_buf(), cfg, Box::new(StdoutSink))
}

fn config_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn run_loop(config_path: PathBuf, cfg: Config, sink: Box<dyn PresenceSink>) -> Result<()> {
    install_signal_handler();

    let poll = Duration::from_secs(cfg.poll_interval_secs.max(1));
    let debounce_min = Duration::from_secs(cfg.min_push_interval_secs.max(1));
    let debouncer = Debouncer::new(debounce_min);

    let state = RunState {
        config_mtime: config_mtime(&config_path),
        config_path,
        cfg,
        sink,
        sys: System::new(),
        debouncer,
        backoff: RetryBackoff::default(),
        last_agent_id: None,
        session_start_unix: None,
        poll,
        first_tick: true,
    };
    run_forever(state);
    Ok(())
}

struct RunState {
    config_path: PathBuf,
    /// Last-seen modification time, for change detection without a filesystem watcher.
    config_mtime: Option<SystemTime>,
    cfg: Config,
    sink: Box<dyn PresenceSink>,
    sys: System,
    debouncer: Debouncer,
    /// Paces retries after a failed push, so a closed Discord is not reopened every poll interval.
    backoff: RetryBackoff,
    last_agent_id: Option<String>,
    session_start_unix: Option<u64>,
    poll: Duration,
    first_tick: bool,
}

/// Reload the config if the file changed on disk.
///
/// Returns true when a new config was applied, so the caller can force a push and make the change
/// visible immediately. A config that no longer loads is reported and **ignored** — a typo in an edit
/// must not take down a running daemon.
fn maybe_reload_config(state: &mut RunState) -> bool {
    let current = config_mtime(&state.config_path);
    if current == state.config_mtime {
        return false;
    }
    // Record the new mtime either way, so a persistently invalid file is not re-reported every tick.
    state.config_mtime = current;

    let next = match Config::load_from_path(&state.config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(
                error = %format!("{e:#}"),
                path = %state.config_path.display(),
                "config changed but does not load; keeping the previous configuration"
            );
            return false;
        }
    };

    // The Discord connection is bound to the client id at startup; changing it needs a reconnect.
    if next.discord.client_id != state.cfg.discord.client_id {
        warn!("discord.client_id changed; restart devsignal for that to take effect");
    }

    state.poll = Duration::from_secs(next.poll_interval_secs.max(1));
    state
        .debouncer
        .set_min_interval(Duration::from_secs(next.min_push_interval_secs.max(1)));
    state.cfg = next;
    info!(path = %state.config_path.display(), "config reloaded");
    true
}

/// One attempt at telling Discord something.
///
/// Ordering matters. The retry backoff is consulted first, then the debouncer, then the sink — and the
/// debouncer is told the send happened only once the sink confirms it. Recording an unconfirmed send is
/// what used to wedge the daemon: a failed push was marked as delivered, the next tick deduplicated
/// against it, and the sink was never called again, so the reconnect inside it never fired.
fn push(state: &mut RunState, action: PresenceAction<'_>, force: bool) {
    let now = Instant::now();
    if !state.backoff.ready(now) {
        debug!(
            failures = state.backoff.consecutive_failures(),
            "skipping presence push; waiting out the retry backoff"
        );
        return;
    }

    if !state.debouncer.may_send(action, force) {
        if force {
            // A refused *forced* send can only be the rate limit: force already skips the equality
            // check and the minimum interval.
            warn!("presence update suppressed by the Discord rate limit; is an agent process flapping?");
        }
        return;
    }

    let outcome = match action {
        PresenceAction::Set(view) => {
            debug!(name = ?view.name, details = ?view.details, state = ?view.state, "pushing presence");
            state.sink.set(view)
        }
        PresenceAction::Clear => {
            debug!("clearing presence");
            state.sink.clear()
        }
    };

    match outcome {
        Ok(()) => {
            state.debouncer.record_sent(action);
            // Only announce a recovery if there was something to recover from, so the healthy path
            // stays silent.
            if state.backoff.consecutive_failures() > 0 {
                info!(
                    failures = state.backoff.consecutive_failures(),
                    "Discord presence recovered"
                );
            }
            state.backoff.record_success();
        }
        Err(e) => {
            state.backoff.record_failure(now);
            // Log the transition into failure, not every retry: with Discord closed all day this
            // would otherwise be the daemon's main output, and the log file has no rotation.
            if state.backoff.consecutive_failures() == 1 {
                warn!(
                    error = %format!("{e:#}"),
                    retry_in_secs = state.backoff.retry_delay(Instant::now()).as_secs_f32(),
                    "presence push failed; will keep retrying"
                );
            } else {
                debug!(
                    error = %format!("{e:#}"),
                    failures = state.backoff.consecutive_failures(),
                    "presence push still failing"
                );
            }
        }
    }
}

/// One iteration of the poll loop, minus the sleep.
///
/// Extracted from `run_forever` so the failure paths can be driven directly from tests, without
/// subprocesses, signals, or touching the `RUNNING` static.
fn tick(state: &mut RunState) {
    let reloaded = maybe_reload_config(state);
    refresh_processes(&mut state.sys, state.cfg.show_cwd_basename);

    let candidates = collect_matches(&state.sys, &state.cfg);
    let selected = winner_of(&candidates);

    let agent_id = selected.as_ref().map(|(a, _)| a.id.clone());
    let transition = agent_id != state.last_agent_id;
    if transition {
        debug!(?agent_id, candidates = candidates.len(), "agent transition");
    }

    let force = transition || state.first_tick || reloaded;
    state.first_tick = false;

    if selected.is_none() && state.cfg.idle_mode == IdleMode::Clear {
        push(state, PresenceAction::Clear, force);
        if transition {
            state.session_start_unix = None;
            state.last_agent_id = agent_id;
        }
        return;
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

    let (view, policy) = build_policy_view(
        &state.cfg,
        selected.as_ref().map(|(a, _)| a),
        bundle.as_deref(),
        state.session_start_unix,
        cwd_hint.as_deref(),
        Some(local_minutes_now()),
    );
    debug!(matched_rule = ?policy.matched_rule_name, "policy applied");

    push(state, PresenceAction::Set(&view), force);
}

fn run_forever(mut state: RunState) {
    while RUNNING.load(Ordering::SeqCst) {
        tick(&mut state);
        sleep_interruptible(state.poll);
    }

    // The one legitimate bypass of the debouncer: shutdown must always clear, rate limit or not.
    // This is the whole point of trapping SIGTERM. Deliberately *not* routed through `push` — the
    // backoff gate could skip it, and retrying would delay exit.
    if let Err(e) = state.sink.clear() {
        warn!(
            error = %format!("{e:#}"),
            "final clear failed; presence may be left stale in Discord"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(extra: &str) -> Config {
        let toml = format!(
            "{extra}\n[discord]\nclient_id = \"1\"\n\n\
             [[agents]]\nid = \"claude_code\"\nprocess_names = [\"claude\"]\n"
        );
        toml::from_str(&toml).expect("test config parses")
    }

    fn test_agent() -> ActiveAgent {
        ActiveAgent {
            id: "claude_code".into(),
            label: "Claude Code".into(),
            large_image: "claude_code".into(),
            small_image: None,
            small_text: None,
            buttons: vec![],
        }
    }

    /// A `RunState` wired to a test sink, pointing at a config path that does not exist.
    ///
    /// A missing config file is deliberate: `maybe_reload_config` reads its mtime as `None` on every
    /// tick, which compares equal and so never triggers a reload. That keeps these tests about the
    /// push path.
    fn test_run_state(cfg: Config, sink: Box<dyn PresenceSink>, backoff: RetryBackoff) -> RunState {
        RunState {
            config_path: PathBuf::from("/nonexistent/devsignal-test-config.toml"),
            config_mtime: None,
            cfg,
            sink,
            sys: System::new(),
            // Duration::ZERO so dedupe is governed by payload equality alone, not by timing.
            debouncer: Debouncer::with_limits(Duration::ZERO, 100, Duration::from_secs(60)),
            backoff,
            last_agent_id: None,
            session_start_unix: None,
            poll: Duration::from_secs(1),
            first_tick: true,
        }
    }

    /// The regression test for the wedge. With a failing sink the payload was recorded as sent, so
    /// tick 2 deduplicated against it and never called the sink again — quitting Discord killed
    /// presence until the agent changed.
    ///
    /// Reverting the `record_sent`-on-success split makes this fail with exactly 1 attempt.
    #[test]
    fn a_failed_push_is_retried_on_a_later_tick() {
        let (sink, log) = sink::ScriptedSink::new(&[false]);
        let mut state = test_run_state(
            test_config(""),
            Box::new(sink),
            RetryBackoff::new(Duration::ZERO, Duration::ZERO),
        );

        for _ in 0..3 {
            tick(&mut state);
        }

        assert_eq!(
            log.borrow().sets,
            3,
            "every tick must retry while the sink is failing"
        );
    }

    /// The `idle_mode = "clear"` branch had the identical bug and is reached by a different path, so
    /// it needs its own guard.
    #[test]
    fn a_failed_clear_is_retried_on_a_later_tick() {
        let (sink, log) = sink::ScriptedSink::new(&[false]);
        // No agent process named "definitely-not-running" exists, so the loop takes the idle branch.
        let cfg: Config = toml::from_str(
            "idle_mode = \"clear\"\n[discord]\nclient_id = \"1\"\n\n\
             [[agents]]\nid = \"nope\"\nprocess_names = [\"definitely-not-running-xyzzy\"]\n",
        )
        .expect("test config parses");
        let mut state = test_run_state(
            cfg,
            Box::new(sink),
            RetryBackoff::new(Duration::ZERO, Duration::ZERO),
        );

        for _ in 0..3 {
            tick(&mut state);
        }

        assert_eq!(log.borrow().clears, 3, "the clear path must retry too");
        assert_eq!(log.borrow().sets, 0, "no agent matched, so nothing to set");
    }

    /// The failure mode the two-phase debouncer introduces: forget `record_sent` and the daemon writes
    /// to Discord every single tick. An unchanged payload must still be sent exactly once.
    #[test]
    fn a_successful_push_is_still_debounced() {
        let (sink, log) = sink::ScriptedSink::new(&[true]);
        let mut state = test_run_state(test_config(""), Box::new(sink), RetryBackoff::default());

        for _ in 0..3 {
            tick(&mut state);
        }

        assert_eq!(
            log.borrow().sets,
            1,
            "an unchanged view must be deduplicated after a confirmed send"
        );
    }

    /// Retrying is right; retrying every `poll_interval_secs` against a Discord that is simply closed
    /// is not. A real backoff must collapse those ticks into one attempt.
    #[test]
    fn backoff_suppresses_retries_while_discord_is_down() {
        let (sink, log) = sink::ScriptedSink::new(&[false]);
        let mut state = test_run_state(
            test_config(""),
            Box::new(sink),
            RetryBackoff::new(Duration::from_secs(10), Duration::from_secs(60)),
        );

        for _ in 0..5 {
            tick(&mut state);
        }

        assert_eq!(
            log.borrow().sets,
            1,
            "the first failure closes the gate for 10s, so the other four ticks skip"
        );
    }

    /// Recovery needs no dedicated code path: once the sink works, whatever the current view is gets
    /// published. This pins that down, including that the published view is the real one.
    #[test]
    fn presence_is_republished_after_the_sink_recovers() {
        let (sink, log) = sink::ScriptedSink::new(&[false, false, true]);
        let mut state = test_run_state(
            test_config(""),
            Box::new(sink),
            RetryBackoff::new(Duration::ZERO, Duration::ZERO),
        );

        for _ in 0..3 {
            tick(&mut state);
        }

        assert_eq!(log.borrow().sets, 3, "two failures then a success");
        let views = &log.borrow().sent_views;
        assert_eq!(views.len(), 1, "only the successful send is recorded");
        assert!(
            views[0].details.is_some() || views[0].name.is_some() || views[0].state.is_some(),
            "the recovered send must carry a real payload, not an empty one"
        );
    }

    /// `hide_host` has to survive both routes into it — a rule's `then`, and
    /// `platforms.disabled_hosts` — and neither may leak the app name it is hiding.
    #[test]
    fn hide_host_replaces_the_host_line_from_rule_or_platforms() {
        let agent = test_agent();
        let ghostty = Some("com.mitchellh.ghostty");

        let by_rule = test_config(
            "show_cwd_basename = true\n\n\
             [[rules]]\nname = \"private\"\nthen = { hide_host = true }\n",
        );
        let (view, policy) = build_policy_view(
            &by_rule,
            Some(&agent),
            ghostty,
            None,
            Some("myrepo"),
            Some(600),
        );
        assert_eq!(policy.matched_rule_name.as_deref(), Some("private"));
        assert_eq!(view.state.as_deref(), Some("Working · myrepo"));

        let by_platforms =
            test_config("[platforms]\ndisabled_hosts = [\"com.mitchellh.ghostty\"]\n");
        let (view, _) = build_policy_view(&by_platforms, Some(&agent), ghostty, None, None, None);
        assert_eq!(view.state.as_deref(), Some("Working"));

        // Idle with the host hidden still has to say something true.
        let (idle, _) = build_policy_view(&by_platforms, None, ghostty, None, None, None);
        assert_eq!(idle.state.as_deref(), Some("No agent CLI detected"));
    }

    /// Hiding the host label must hide its icon too, or `hide_host` leaks the app through the
    /// small image instead of the text.
    #[test]
    fn hide_host_also_suppresses_the_host_icon() {
        let agent = test_agent();
        let cfg = test_config(
            "[images]\nmode = \"url\"\nhost_icon = true\n\n\
             [platforms]\ndisabled_hosts = [\"com.mitchellh.ghostty\"]\n",
        );
        let (shown, _) = build_policy_view(
            &cfg,
            Some(&agent),
            Some("com.apple.Terminal"),
            None,
            None,
            None,
        );
        assert_eq!(
            shown.small_image.as_deref(),
            Some("https://raw.githubusercontent.com/rabbive/devsignal/main/assets/discord/hosts/terminal.png")
        );

        let (hidden, _) = build_policy_view(
            &cfg,
            Some(&agent),
            Some("com.mitchellh.ghostty"),
            None,
            None,
            None,
        );
        assert!(hidden.small_image.is_none());
        assert!(hidden.small_text.is_none());
    }

    /// The reordering this exists for: agent on line 1, host on line 2, brand on line 3.
    #[test]
    fn agent_first_layout_moves_the_agent_to_line_one() {
        let cfg =
            test_config("[presence]\nname = \"agent\"\ndetails = \"host\"\nstate = \"brand\"\n");
        let (view, _) = build_policy_view(
            &cfg,
            Some(&test_agent()),
            Some("com.mitchellh.ghostty"),
            None,
            None,
            None,
        );
        assert_eq!(view.name.as_deref(), Some("Claude Code"));
        assert_eq!(view.details.as_deref(), Some("In Ghostty"));
        assert_eq!(view.state.as_deref(), Some("devsignal"));
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
