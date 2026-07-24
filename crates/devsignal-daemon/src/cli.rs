//! Command-line parsing.
//!
//! Deliberately free of `#[cfg(target_os = ...)]` so it stays lintable and testable on every
//! platform: parsing never touches AppKit, launchd, or Discord. Platform gating lives at the point
//! of use (`run_daemon`, `cmd_init`), not here.

use anyhow::{Context, Result};
use devsignal_core::Config;
use std::path::PathBuf;

use crate::config_edit::{
    parse_agents_command, parse_hosts_command, parse_rules_command, ConfigEditCommand,
};

#[derive(Debug)]
pub enum Cli {
    Run(RunArgs),
    Validate { config: PathBuf },
    Once { config: PathBuf },
    Detect { config: PathBuf },
    Watch { config: PathBuf },
    Init { config: PathBuf },
    ConfigEdit(ConfigEditCommand),
    Help,
    Version,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RunArgs {
    pub config: PathBuf,
    /// When true, retry Discord IPC until timeout if Discord is not running.
    pub wait_for_discord: bool,
}

/// Usage text. Returned rather than printed so callers pick the stream: an explicitly requested
/// `--help` belongs on stdout, a usage dump after a parse error belongs on stderr.
pub fn global_help() -> String {
    format!(
        "devsignal {} — unified Discord Rich Presence for AI coding CLIs (macOS)\n\
         \n\
         Usage:\n\
           devsignal [run] [options]\n\
           devsignal init     [-c path]   Guided setup wizard\n\
           devsignal validate [-c path]   Parse and check the config, then exit\n\
           devsignal once     [-c path]   Print the presence payload as JSON (no Discord)\n\
           devsignal detect   [-c path]   Show which agent processes match, and which wins\n\
           devsignal watch    [-c path]   Run the poll loop, printing instead of using Discord\n\
           devsignal hosts  list | enable <bundle_id> | disable <bundle_id>\n\
           devsignal agents list | enable <agent_id>  | disable <agent_id>\n\
           devsignal rules  list | remove <name> | add --name <name> [rule flags]\n\
           devsignal help | --help | -h\n\
           devsignal version | --version | -V\n\
         \n\
         Every subcommand accepts -c/--config <path>. Default config:\n\
           {}\n\
         \n\
         Run options:\n\
           -c, --config <path>     Config file (default: see above)\n\
           --wait-for-discord      Retry until Discord is available (default)\n\
           --no-wait-for-discord   Fail immediately if Discord IPC is unavailable\n\
         \n\
         Rule flags (devsignal rules add):\n\
           --name <name>           Rule name (required)\n\
           --host <bundle_id>      Match this frontmost host; repeatable\n\
           --agent <agent_id>      Match this agent id; repeatable\n\
           --project <basename>    Match this project directory name; repeatable\n\
           --time <HH:MM-HH:MM>    Match this local time window (may wrap past midnight)\n\
           --active-only           Match only while an agent is running\n\
           --idle-only             Match only while no agent is running\n\
           --hide-host             Omit the host label from the presence\n\
           --state <text>          Replace the presence state line\n\
         At least one of --hide-host / --state is required.\n",
        env!("CARGO_PKG_VERSION"),
        Config::default_path().display()
    )
}

pub fn version_line() -> String {
    format!("devsignal {}", env!("CARGO_PKG_VERSION"))
}

fn is_help_flag(arg: &str) -> bool {
    matches!(arg, "--help" | "-h" | "help")
}

fn is_version_flag(arg: &str) -> bool {
    matches!(arg, "--version" | "-V" | "version")
}

/// Parse `-c/--config <path>` and nothing else. Used by subcommands that take no other options.
fn parse_config_path_only(args: &[String]) -> Result<PathBuf> {
    let mut path = Config::default_path();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" | "-c" => {
                let p = it.next().context("--config requires a path")?;
                path = PathBuf::from(p);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(path)
}

fn parse_run_args(args: &[String]) -> Result<RunArgs> {
    let mut path = Config::default_path();
    let mut wait_for_discord = true;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" | "-c" => {
                let p = it.next().context("--config requires a path")?;
                path = PathBuf::from(p);
            }
            "--wait-for-discord" => wait_for_discord = true,
            "--no-wait-for-discord" => wait_for_discord = false,
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(RunArgs {
        config: path,
        wait_for_discord,
    })
}

/// Parse a full argument vector (already stripped of argv[0]).
pub fn parse_cli(args: &[String]) -> Result<Cli> {
    let Some(first) = args.first() else {
        return Ok(Cli::Run(RunArgs {
            config: Config::default_path(),
            wait_for_discord: true,
        }));
    };

    // A bare help/version flag anywhere in a subcommand's arguments wins over parsing, so
    // `devsignal rules add --help` prints usage instead of dying on an unknown flag.
    if args.iter().any(|a| is_help_flag(a)) {
        return Ok(Cli::Help);
    }
    if args.iter().any(|a| is_version_flag(a)) {
        return Ok(Cli::Version);
    }

    let rest = &args[1..];
    match first.as_str() {
        "init" => Ok(Cli::Init {
            config: parse_config_path_only(rest)?,
        }),
        "validate" => Ok(Cli::Validate {
            config: parse_config_path_only(rest)?,
        }),
        "once" => Ok(Cli::Once {
            config: parse_config_path_only(rest)?,
        }),
        "detect" => Ok(Cli::Detect {
            config: parse_config_path_only(rest)?,
        }),
        "watch" => Ok(Cli::Watch {
            config: parse_config_path_only(rest)?,
        }),
        "run" => Ok(Cli::Run(parse_run_args(rest)?)),
        "hosts" => Ok(Cli::ConfigEdit(parse_hosts_command(rest)?)),
        "agents" => Ok(Cli::ConfigEdit(parse_agents_command(rest)?)),
        "rules" => Ok(Cli::ConfigEdit(parse_rules_command(rest)?)),
        // Legacy: `devsignal --config foo` with no subcommand.
        other if other.starts_with('-') => Ok(Cli::Run(parse_run_args(args)?)),
        other => anyhow::bail!(
            "unknown subcommand: {other}\nRun `devsignal --help` to see available commands."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_args_default_to_run_with_wait() {
        let cli = parse_cli(&[]).expect("parse");
        match cli {
            Cli::Run(args) => {
                assert!(args.wait_for_discord);
                assert_eq!(args.config, Config::default_path());
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn help_and_version_flags_map_to_variants() {
        for flag in ["--help", "-h", "help"] {
            assert!(matches!(
                parse_cli(&argv(&[flag])).expect("parse"),
                Cli::Help
            ));
        }
        for flag in ["--version", "-V", "version"] {
            assert!(matches!(
                parse_cli(&argv(&[flag])).expect("parse"),
                Cli::Version
            ));
        }
    }

    #[test]
    fn help_flag_wins_inside_a_subcommand() {
        // Regression: `rules add --help` used to fail with "unknown rules add flag: --help".
        assert!(matches!(
            parse_cli(&argv(&["rules", "add", "--help"])).expect("parse"),
            Cli::Help
        ));
        assert!(matches!(
            parse_cli(&argv(&["hosts", "--help"])).expect("parse"),
            Cli::Help
        ));
    }

    #[test]
    fn subcommands_accept_config_flag() {
        let cases = ["validate", "once", "detect", "watch", "init"];
        for name in cases {
            let cli = parse_cli(&argv(&[name, "--config", "/tmp/x.toml"])).expect("parse");
            let got = match cli {
                Cli::Validate { config }
                | Cli::Once { config }
                | Cli::Detect { config }
                | Cli::Watch { config }
                | Cli::Init { config } => config,
                other => panic!("unexpected variant for {name}: {other:?}"),
            };
            assert_eq!(got, PathBuf::from("/tmp/x.toml"));
        }
    }

    #[test]
    fn run_parses_wait_flags_and_config() {
        let cli = parse_cli(&argv(&[
            "run",
            "-c",
            "/tmp/a.toml",
            "--no-wait-for-discord",
        ]))
        .expect("parse");
        match cli {
            Cli::Run(args) => {
                assert_eq!(args.config, PathBuf::from("/tmp/a.toml"));
                assert!(!args.wait_for_discord);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn legacy_bare_config_flag_is_run() {
        let cli = parse_cli(&argv(&["--config", "/tmp/legacy.toml"])).expect("parse");
        match cli {
            Cli::Run(args) => assert_eq!(args.config, PathBuf::from("/tmp/legacy.toml")),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn unknown_subcommand_points_at_help() {
        let err = parse_cli(&argv(&["ruls"])).expect_err("should fail");
        let msg = format!("{err}");
        assert!(msg.contains("unknown subcommand: ruls"), "got {msg}");
        assert!(msg.contains("--help"), "got {msg}");
    }

    #[test]
    fn config_flag_without_value_errors() {
        assert!(parse_cli(&argv(&["validate", "--config"])).is_err());
        assert!(parse_cli(&argv(&["run", "-c"])).is_err());
    }

    #[test]
    fn help_and_version_text_carry_the_crate_version() {
        let v = env!("CARGO_PKG_VERSION");
        assert!(version_line().contains(v));
        assert!(global_help().contains(v));
        // The rule flags must be documented; they previously existed only in the README.
        assert!(global_help().contains("--hide-host"));
        assert!(global_help().contains("--time <HH:MM-HH:MM>"));
        assert!(global_help().contains("detect"));
        assert!(global_help().contains("watch"));
    }
}
