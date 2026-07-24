use anyhow::{Context, Result};
use devsignal_core::{
    AgentRule, ButtonConfig, Config, PresenceRule, RuleThen, RuleWhen, TimeWindow,
    HOST_BUNDLE_LABELS,
};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum ConfigEditCommand {
    Hosts(HostsCommand),
    Agents(AgentsCommand),
    Rules(RulesCommand),
}

#[derive(Debug)]
pub enum HostsCommand {
    List { config: PathBuf },
    Enable { config: PathBuf, id: String },
    Disable { config: PathBuf, id: String },
}

#[derive(Debug)]
pub enum AgentsCommand {
    List {
        config: PathBuf,
    },
    Enable {
        config: PathBuf,
        id: String,
    },
    Disable {
        config: PathBuf,
        id: String,
    },
    Add {
        config: PathBuf,
        rule: Box<AgentRule>,
    },
    Remove {
        config: PathBuf,
        id: String,
    },
}

#[derive(Debug)]
pub enum RulesCommand {
    List { config: PathBuf },
    Remove { config: PathBuf, name: String },
    Add { config: PathBuf, rule: PresenceRule },
}

fn take_config(args: &mut Vec<String>) -> Result<PathBuf> {
    let mut config = Config::default_path();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                let value = args.get(i + 1).context("--config requires a path")?.clone();
                config = PathBuf::from(value);
                args.drain(i..=i + 1);
            }
            _ => i += 1,
        }
    }
    Ok(config)
}

pub fn parse_hosts_command(args: &[String]) -> Result<ConfigEditCommand> {
    let mut args = args.to_vec();
    let config = take_config(&mut args)?;
    match args.as_slice() {
        [cmd] if cmd == "list" => Ok(ConfigEditCommand::Hosts(HostsCommand::List { config })),
        [cmd, id] if cmd == "enable" => Ok(ConfigEditCommand::Hosts(HostsCommand::Enable {
            config,
            id: id.clone(),
        })),
        [cmd, id] if cmd == "disable" => Ok(ConfigEditCommand::Hosts(HostsCommand::Disable {
            config,
            id: id.clone(),
        })),
        _ => anyhow::bail!(
            "usage: devsignal hosts list|enable <bundle_id>|disable <bundle_id> [--config path]"
        ),
    }
}

pub fn parse_agents_command(args: &[String]) -> Result<ConfigEditCommand> {
    let mut args = args.to_vec();
    let config = take_config(&mut args)?;
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            Ok(ConfigEditCommand::Agents(AgentsCommand::List { config }))
        }
        Some("enable") if args.len() == 2 => Ok(ConfigEditCommand::Agents(AgentsCommand::Enable {
            config,
            id: args[1].clone(),
        })),
        Some("disable") if args.len() == 2 => {
            Ok(ConfigEditCommand::Agents(AgentsCommand::Disable {
                config,
                id: args[1].clone(),
            }))
        }
        Some("remove") if args.len() == 2 => Ok(ConfigEditCommand::Agents(AgentsCommand::Remove {
            config,
            id: args[1].clone(),
        })),
        Some("add") => Ok(ConfigEditCommand::Agents(AgentsCommand::Add {
            config,
            rule: Box::new(parse_agent_add(&args[1..])?),
        })),
        _ => anyhow::bail!(
            "usage: devsignal agents list | enable <id> | disable <id> | remove <id> |\n\
             \x20      add --id <id> --process-name <name> [--label <text>] [--priority <n>]\n\
             \x20          [--large-image <key>] [--small-image <key>] [--small-text <text>]\n\
             \x20          [--button \"<label>=<url>\"]"
        ),
    }
}

/// Parse `agents add`. Field validation (duplicate ids, button limits, empty process names) is left
/// to `Config::validate`, so the CLI and a hand-edited file are held to exactly the same rules.
fn parse_agent_add(args: &[String]) -> Result<AgentRule> {
    let mut id = None;
    let mut label = None;
    let mut process_names = Vec::new();
    let mut argv_substrings = Vec::new();
    let mut priority = None;
    let mut large_image = None;
    let mut small_image = None;
    let mut small_text = None;
    let mut buttons = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let need_value = |flag: &str| -> Result<String> {
            args.get(i + 1)
                .cloned()
                .with_context(|| format!("{flag} requires a value"))
        };
        match args[i].as_str() {
            "--id" => {
                id = Some(need_value("--id")?);
                i += 2;
            }
            "--label" => {
                label = Some(need_value("--label")?);
                i += 2;
            }
            "--process-name" => {
                process_names.push(need_value("--process-name")?);
                i += 2;
            }
            "--argv-substring" => {
                argv_substrings.push(need_value("--argv-substring")?);
                i += 2;
            }
            "--priority" => {
                let raw = need_value("--priority")?;
                priority = Some(
                    raw.parse::<i32>()
                        .with_context(|| format!("--priority must be a number (got {raw:?})"))?,
                );
                i += 2;
            }
            "--large-image" => {
                large_image = Some(need_value("--large-image")?);
                i += 2;
            }
            "--small-image" => {
                small_image = Some(need_value("--small-image")?);
                i += 2;
            }
            "--small-text" => {
                small_text = Some(need_value("--small-text")?);
                i += 2;
            }
            "--button" => {
                let raw = need_value("--button")?;
                let (label, url) = raw
                    .split_once('=')
                    .context("--button must be formatted as \"<label>=<url>\"")?;
                buttons.push(ButtonConfig {
                    label: label.to_string(),
                    url: url.to_string(),
                });
                i += 2;
            }
            other => anyhow::bail!("unknown agents add flag: {other}"),
        }
    }

    let id = id.context("agents add requires --id <id>")?;
    anyhow::ensure!(
        !process_names.is_empty(),
        "agents add requires at least one --process-name <name>. \n\
         Run `devsignal detect --unmatched` while the CLI is running to find the right name."
    );

    Ok(AgentRule {
        id,
        label,
        process_names,
        argv_substrings,
        large_image,
        // Community presets start at 100 so they do not collide with the shipped band (10/20/30).
        priority: priority.unwrap_or(100),
        small_image,
        small_text,
        buttons,
    })
}

pub fn parse_rules_command(args: &[String]) -> Result<ConfigEditCommand> {
    let mut args = args.to_vec();
    let config = take_config(&mut args)?;
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            Ok(ConfigEditCommand::Rules(RulesCommand::List { config }))
        }
        Some("remove") if args.len() == 2 => Ok(ConfigEditCommand::Rules(RulesCommand::Remove {
            config,
            name: args[1].clone(),
        })),
        Some("add") => Ok(ConfigEditCommand::Rules(RulesCommand::Add {
            config,
            rule: parse_rule_add(&args[1..])?,
        })),
        _ => anyhow::bail!("usage: devsignal rules list|remove <name>|add --name <name> [flags]"),
    }
}

fn parse_rule_add(args: &[String]) -> Result<PresenceRule> {
    let mut name = None;
    let mut when = RuleWhen::default();
    let mut then = RuleThen::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                name = Some(args.get(i + 1).context("--name requires a value")?.clone());
                i += 2;
            }
            "--host" => {
                when.host_bundle_ids
                    .push(args.get(i + 1).context("--host requires a value")?.clone());
                i += 2;
            }
            "--agent" => {
                when.agent_ids
                    .push(args.get(i + 1).context("--agent requires a value")?.clone());
                i += 2;
            }
            "--active-only" => {
                when.active_only = true;
                i += 1;
            }
            "--idle-only" => {
                when.idle_only = true;
                i += 1;
            }
            "--project" => {
                when.project_basenames.push(
                    args.get(i + 1)
                        .context("--project requires a value")?
                        .clone(),
                );
                i += 2;
            }
            "--time" => {
                let raw = args.get(i + 1).context("--time requires HH:MM-HH:MM")?;
                let (start, end) = raw
                    .split_once('-')
                    .context("--time must be formatted as HH:MM-HH:MM")?;
                when.time = Some(TimeWindow {
                    start: start.to_string(),
                    end: end.to_string(),
                });
                i += 2;
            }
            "--hide-host" => {
                then.hide_host = true;
                i += 1;
            }
            "--state" => {
                then.state = Some(args.get(i + 1).context("--state requires a value")?.clone());
                i += 2;
            }
            other => anyhow::bail!("unknown rules add flag: {other}"),
        }
    }
    let name = name.context("rules add requires --name <name>")?;
    anyhow::ensure!(
        then.hide_host || then.state.is_some(),
        "rules add requires --hide-host and/or --state <text>"
    );
    Ok(PresenceRule { name, when, then })
}

fn load_config(path: &Path) -> Result<Config> {
    Config::load_from_path(path).with_context(|| format!("load config {}", path.display()))
}

fn write_config(path: &Path, cfg: &Config) -> Result<()> {
    crate::config_io::write_config_atomic(path, cfg)
}

/// Persist the change, then report it. Rewriting the config from the parsed struct drops comments,
/// and a running daemon reads its config only at startup — say both out loud rather than letting the
/// user wonder why nothing happened.
fn commit(path: &Path, cfg: &Config, message: &str) -> Result<()> {
    write_config(path, cfg)?;
    println!("{message}");
    println!(
        "  wrote {} (comments and key order are not preserved)",
        path.display()
    );
    println!(
        "  restart the daemon to apply: launchctl kickstart -k gui/$(id -u)/com.devsignal.daemon"
    );
    Ok(())
}

fn remove_case_insensitive(items: &mut Vec<String>, value: &str) {
    items.retain(|item| !item.eq_ignore_ascii_case(value));
}

fn add_unique_case_insensitive(items: &mut Vec<String>, value: String) {
    if !items.iter().any(|item| item.eq_ignore_ascii_case(&value)) {
        items.push(value);
    }
}

pub fn run_config_edit(cmd: ConfigEditCommand) -> Result<()> {
    match cmd {
        ConfigEditCommand::Hosts(cmd) => run_hosts(cmd),
        ConfigEditCommand::Agents(cmd) => run_agents(cmd),
        ConfigEditCommand::Rules(cmd) => run_rules(cmd),
    }
}

fn run_hosts(cmd: HostsCommand) -> Result<()> {
    match cmd {
        HostsCommand::List { config } => {
            let cfg = load_config(&config)?;
            for (bundle, label) in HOST_BUNDLE_LABELS {
                let enabled = !cfg
                    .platforms
                    .disabled_hosts
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(bundle));
                println!(
                    "{}\t{}\t{}",
                    if enabled { "enabled" } else { "disabled" },
                    bundle,
                    label
                );
            }
            Ok(())
        }
        HostsCommand::Enable { config, id } => {
            let mut cfg = load_config(&config)?;
            remove_case_insensitive(&mut cfg.platforms.disabled_hosts, &id);
            commit(&config, &cfg, &format!("enabled host: {id}"))
        }
        HostsCommand::Disable { config, id } => {
            let mut cfg = load_config(&config)?;
            add_unique_case_insensitive(&mut cfg.platforms.disabled_hosts, id.clone());
            commit(&config, &cfg, &format!("disabled host: {id}"))
        }
    }
}

fn run_agents(cmd: AgentsCommand) -> Result<()> {
    match cmd {
        AgentsCommand::List { config } => {
            let cfg = load_config(&config)?;
            for agent in &cfg.agents {
                let enabled = !cfg
                    .platforms
                    .disabled_agents
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&agent.id));
                println!(
                    "{}\t{}\t{}",
                    if enabled { "enabled" } else { "disabled" },
                    agent.id,
                    agent.label.as_deref().unwrap_or(&agent.id)
                );
            }
            Ok(())
        }
        AgentsCommand::Enable { config, id } => {
            let mut cfg = load_config(&config)?;
            remove_case_insensitive(&mut cfg.platforms.disabled_agents, &id);
            commit(&config, &cfg, &format!("enabled agent: {id}"))
        }
        AgentsCommand::Disable { config, id } => {
            let mut cfg = load_config(&config)?;
            add_unique_case_insensitive(&mut cfg.platforms.disabled_agents, id.clone());
            commit(&config, &cfg, &format!("disabled agent: {id}"))
        }
        AgentsCommand::Add { config, rule } => {
            let mut cfg = load_config(&config)?;
            let id = rule.id.clone();
            let names = rule.process_names.join(", ");
            cfg.agents.push(*rule);
            // Config::validate rejects a duplicate id, so no separate check is needed here.
            commit(
                &config,
                &cfg,
                &format!("added agent: {id} (matching: {names})"),
            )?;
            println!("  verify with: devsignal detect");
            Ok(())
        }
        AgentsCommand::Remove { config, id } => {
            let mut cfg = load_config(&config)?;
            let before = cfg.agents.len();
            cfg.agents.retain(|a| !a.id.eq_ignore_ascii_case(&id));
            anyhow::ensure!(cfg.agents.len() != before, "agent not found: {id}");
            // Leaving a stale entry in disabled_agents would silently suppress a future re-add.
            remove_case_insensitive(&mut cfg.platforms.disabled_agents, &id);
            commit(&config, &cfg, &format!("removed agent: {id}"))
        }
    }
}

fn run_rules(cmd: RulesCommand) -> Result<()> {
    match cmd {
        RulesCommand::List { config } => {
            let cfg = load_config(&config)?;
            if cfg.rules.is_empty() {
                println!("no rules configured");
            }
            for rule in &cfg.rules {
                println!("{}\t{:?}\t{:?}", rule.name, rule.when, rule.then);
            }
            Ok(())
        }
        RulesCommand::Remove { config, name } => {
            let mut cfg = load_config(&config)?;
            let before = cfg.rules.len();
            cfg.rules.retain(|rule| rule.name != name);
            anyhow::ensure!(cfg.rules.len() != before, "rule not found: {name}");
            commit(&config, &cfg, &format!("removed rule: {name}"))
        }
        RulesCommand::Add { config, rule } => {
            let mut cfg = load_config(&config)?;
            anyhow::ensure!(
                !cfg.rules.iter().any(|r| r.name == rule.name),
                "rule already exists: {}",
                rule.name
            );
            let name = rule.name.clone();
            cfg.rules.push(rule);
            commit(&config, &cfg, &format!("added rule: {name}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn take_config_extracts_the_flag_from_anywhere_in_the_args() {
        let mut args = argv(&["disable", "--config", "/tmp/a.toml", "com.apple.Terminal"]);
        let path = take_config(&mut args).expect("parse");
        assert_eq!(path, PathBuf::from("/tmp/a.toml"));
        // The flag and its value are consumed, leaving the positional arguments.
        assert_eq!(args, argv(&["disable", "com.apple.Terminal"]));
    }

    #[test]
    fn take_config_defaults_and_rejects_a_missing_value() {
        let mut args = argv(&["list"]);
        assert_eq!(
            take_config(&mut args).expect("parse"),
            Config::default_path()
        );

        let mut dangling = argv(&["list", "--config"]);
        assert!(take_config(&mut dangling).is_err());
    }

    #[test]
    fn hosts_and_agents_commands_parse_their_verbs() {
        assert!(matches!(
            parse_hosts_command(&argv(&["list"])).expect("parse"),
            ConfigEditCommand::Hosts(HostsCommand::List { .. })
        ));
        match parse_agents_command(&argv(&["disable", "goose"])).expect("parse") {
            ConfigEditCommand::Agents(AgentsCommand::Disable { id, .. }) => assert_eq!(id, "goose"),
            other => panic!("unexpected: {other:?}"),
        }
        // Missing or extra positional arguments produce the usage error.
        assert!(parse_hosts_command(&argv(&["enable"])).is_err());
        assert!(parse_agents_command(&argv(&["bogus", "x"])).is_err());
    }

    #[test]
    fn rule_add_collects_repeatable_and_boolean_flags() {
        let cmd = parse_rules_command(&argv(&[
            "add",
            "--name",
            "focus",
            "--host",
            "com.apple.Terminal",
            "--host",
            "com.googlecode.iterm2",
            "--agent",
            "claude_code",
            "--project",
            "devsignal",
            "--active-only",
            "--hide-host",
            "--state",
            "Deep work",
        ]))
        .expect("parse");

        let rule = match cmd {
            ConfigEditCommand::Rules(RulesCommand::Add { rule, .. }) => rule,
            other => panic!("unexpected: {other:?}"),
        };
        assert_eq!(rule.name, "focus");
        assert_eq!(rule.when.host_bundle_ids.len(), 2, "--host is repeatable");
        assert_eq!(rule.when.agent_ids, vec!["claude_code".to_string()]);
        assert_eq!(rule.when.project_basenames, vec!["devsignal".to_string()]);
        assert!(rule.when.active_only);
        assert!(!rule.when.idle_only);
        assert!(rule.then.hide_host);
        assert_eq!(rule.then.state.as_deref(), Some("Deep work"));
    }

    #[test]
    fn rule_add_parses_a_time_window() {
        let cmd = parse_rules_command(&argv(&[
            "add",
            "--name",
            "night",
            "--time",
            "22:00-06:00",
            "--hide-host",
        ]))
        .expect("parse");
        let rule = match cmd {
            ConfigEditCommand::Rules(RulesCommand::Add { rule, .. }) => rule,
            other => panic!("unexpected: {other:?}"),
        };
        let window = rule.when.time.expect("time window");
        assert_eq!(window.start, "22:00");
        assert_eq!(window.end, "06:00");
        window.validate().expect("window should be valid");
    }

    #[test]
    fn rule_add_requires_a_name_and_an_effect() {
        // No --name.
        assert!(parse_rules_command(&argv(&["add", "--hide-host"])).is_err());
        // Neither --hide-host nor --state: the rule would match and do nothing.
        let err = parse_rules_command(&argv(&["add", "--name", "noop"])).expect_err("should fail");
        assert!(format!("{err}").contains("--hide-host"));
        // Unknown flag.
        assert!(parse_rules_command(&argv(&["add", "--name", "x", "--nope"])).is_err());
        // Flags that require a value but have none.
        assert!(parse_rules_command(&argv(&["add", "--name"])).is_err());
        assert!(parse_rules_command(&argv(&["add", "--name", "x", "--time"])).is_err());
    }

    #[test]
    fn rule_add_rejects_a_malformed_time_flag() {
        // No separator at all is caught at parse time.
        assert!(parse_rules_command(&argv(&[
            "add",
            "--name",
            "x",
            "--time",
            "2200",
            "--hide-host"
        ]))
        .is_err());
    }

    #[test]
    fn rules_list_and_remove_parse() {
        assert!(matches!(
            parse_rules_command(&argv(&["list"])).expect("parse"),
            ConfigEditCommand::Rules(RulesCommand::List { .. })
        ));
        match parse_rules_command(&argv(&["remove", "focus"])).expect("parse") {
            ConfigEditCommand::Rules(RulesCommand::Remove { name, .. }) => {
                assert_eq!(name, "focus")
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert!(parse_rules_command(&argv(&["remove"])).is_err());
    }

    #[test]
    fn agent_add_collects_every_field() {
        let cmd = parse_agents_command(&argv(&[
            "add",
            "--id",
            "gemini_cli",
            "--label",
            "Gemini CLI",
            "--process-name",
            "gemini",
            "--process-name",
            "gemini-cli",
            "--argv-substring",
            "gemini",
            "--priority",
            "105",
            "--large-image",
            "gemini_key",
            "--small-image",
            "devsignal",
            "--small-text",
            "devsignal",
            "--button",
            "Docs=https://example.com/docs",
        ]))
        .expect("parse");

        let rule = match cmd {
            ConfigEditCommand::Agents(AgentsCommand::Add { rule, .. }) => *rule,
            other => panic!("unexpected: {other:?}"),
        };
        assert_eq!(rule.id, "gemini_cli");
        assert_eq!(rule.label.as_deref(), Some("Gemini CLI"));
        assert_eq!(rule.process_names, vec!["gemini", "gemini-cli"]);
        assert_eq!(rule.argv_substrings, vec!["gemini"]);
        assert_eq!(rule.priority, 105);
        assert_eq!(rule.large_image.as_deref(), Some("gemini_key"));
        assert_eq!(rule.small_text.as_deref(), Some("devsignal"));
        assert_eq!(rule.buttons.len(), 1);
        assert_eq!(rule.buttons[0].label, "Docs");
        assert_eq!(rule.buttons[0].url, "https://example.com/docs");
    }

    /// A URL containing '=' (query strings do) must not be truncated by the split.
    #[test]
    fn agent_add_button_splits_on_the_first_equals_only() {
        let cmd = parse_agents_command(&argv(&[
            "add",
            "--id",
            "x",
            "--process-name",
            "xx",
            "--button",
            "Issue=https://example.com/q?a=1&b=2",
        ]))
        .expect("parse");
        let rule = match cmd {
            ConfigEditCommand::Agents(AgentsCommand::Add { rule, .. }) => *rule,
            other => panic!("unexpected: {other:?}"),
        };
        assert_eq!(rule.buttons[0].url, "https://example.com/q?a=1&b=2");
    }

    #[test]
    fn agent_add_defaults_priority_clear_of_the_shipped_band() {
        let cmd = parse_agents_command(&argv(&["add", "--id", "x", "--process-name", "xx"]))
            .expect("parse");
        let rule = match cmd {
            ConfigEditCommand::Agents(AgentsCommand::Add { rule, .. }) => *rule,
            other => panic!("unexpected: {other:?}"),
        };
        // Shipped presets occupy 10/20/30; a user-added agent must not silently outrank them.
        assert!(rule.priority >= 100, "got {}", rule.priority);
    }

    #[test]
    fn agent_add_requires_id_and_process_name() {
        assert!(parse_agents_command(&argv(&["add", "--process-name", "x"])).is_err());

        let err = parse_agents_command(&argv(&["add", "--id", "x"])).expect_err("should fail");
        let msg = format!("{err}");
        assert!(msg.contains("--process-name"), "got {msg}");
        // The error should send the user to the tool that finds the right name.
        assert!(msg.contains("detect --unmatched"), "got {msg}");

        assert!(parse_agents_command(&argv(&["add", "--id"])).is_err());
        assert!(parse_agents_command(&argv(&["add", "--id", "x", "--nope"])).is_err());
    }

    #[test]
    fn agent_add_rejects_a_non_numeric_priority_and_malformed_button() {
        assert!(parse_agents_command(&argv(&[
            "add",
            "--id",
            "x",
            "--process-name",
            "xx",
            "--priority",
            "high"
        ]))
        .is_err());
        assert!(parse_agents_command(&argv(&[
            "add",
            "--id",
            "x",
            "--process-name",
            "xx",
            "--button",
            "no-equals-sign"
        ]))
        .is_err());
    }

    #[test]
    fn agents_remove_parses() {
        match parse_agents_command(&argv(&["remove", "goose"])).expect("parse") {
            ConfigEditCommand::Agents(AgentsCommand::Remove { id, .. }) => assert_eq!(id, "goose"),
            other => panic!("unexpected: {other:?}"),
        }
        assert!(parse_agents_command(&argv(&["remove"])).is_err());
    }

    #[test]
    fn disabled_list_helpers_are_case_insensitive_and_idempotent() {
        let mut items = vec!["com.apple.Terminal".to_string()];

        add_unique_case_insensitive(&mut items, "COM.APPLE.TERMINAL".into());
        assert_eq!(items.len(), 1, "must not add a case-variant duplicate");

        add_unique_case_insensitive(&mut items, "dev.zed.Zed".into());
        assert_eq!(items.len(), 2);

        remove_case_insensitive(&mut items, "com.APPLE.terminal");
        assert_eq!(items, vec!["dev.zed.Zed".to_string()]);
    }
}
