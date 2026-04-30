use anyhow::{Context, Result};
use devsignal_core::{Config, PresenceRule, RuleThen, RuleWhen, TimeWindow, HOST_BUNDLE_LABELS};
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
    List { config: PathBuf },
    Enable { config: PathBuf, id: String },
    Disable { config: PathBuf, id: String },
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
    match args.as_slice() {
        [cmd] if cmd == "list" => Ok(ConfigEditCommand::Agents(AgentsCommand::List { config })),
        [cmd, id] if cmd == "enable" => Ok(ConfigEditCommand::Agents(AgentsCommand::Enable {
            config,
            id: id.clone(),
        })),
        [cmd, id] if cmd == "disable" => Ok(ConfigEditCommand::Agents(AgentsCommand::Disable {
            config,
            id: id.clone(),
        })),
        _ => anyhow::bail!(
            "usage: devsignal agents list|enable <agent_id>|disable <agent_id> [--config path]"
        ),
    }
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
    let toml = toml::to_string_pretty(cfg).context("serialize config")?;
    std::fs::write(path, toml).with_context(|| format!("write config {}", path.display()))?;
    let _ = Config::load_from_path(path).context("validate rewritten config")?;
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
            write_config(&config, &cfg)?;
            println!("enabled host: {id}");
            Ok(())
        }
        HostsCommand::Disable { config, id } => {
            let mut cfg = load_config(&config)?;
            add_unique_case_insensitive(&mut cfg.platforms.disabled_hosts, id.clone());
            write_config(&config, &cfg)?;
            println!("disabled host: {id}");
            Ok(())
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
            write_config(&config, &cfg)?;
            println!("enabled agent: {id}");
            Ok(())
        }
        AgentsCommand::Disable { config, id } => {
            let mut cfg = load_config(&config)?;
            add_unique_case_insensitive(&mut cfg.platforms.disabled_agents, id.clone());
            write_config(&config, &cfg)?;
            println!("disabled agent: {id}");
            Ok(())
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
            write_config(&config, &cfg)?;
            println!("removed rule: {name}");
            Ok(())
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
            write_config(&config, &cfg)?;
            println!("added rule: {name}");
            Ok(())
        }
    }
}
