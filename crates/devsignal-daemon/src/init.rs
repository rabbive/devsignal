use anyhow::{Context, Result};
use console::style;
use devsignal_core::{AgentRule, ButtonConfig, Config, DiscordSection, IdleMode};
use dialoguer::{Confirm, Input, MultiSelect, Select};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrivacyPreset {
    Minimal,
    ProjectSafe,
    PublicOss,
    Custom,
}

fn banner() -> &'static str {
    r#"██████╗ ███████╗██╗   ██╗███████╗██╗ ██████╗ ███╗   ██╗ █████╗ ██╗
██╔══██╗██╔════╝██║   ██║██╔════╝██║██╔════╝ ████╗  ██║██╔══██╗██║
██║  ██║█████╗  ██║   ██║███████╗██║██║  ███╗██╔██╗ ██║███████║██║
██║  ██║██╔══╝  ╚██╗ ██╔╝╚════██║██║██║   ██║██║╚██╗██║██╔══██║██║
██████╔╝███████╗ ╚████╔╝ ███████║██║╚██████╔╝██║ ╚████║██║  ██║███████╗
╚═════╝ ╚══════╝  ╚═══╝  ╚══════╝╚═╝ ╚═════╝ ╚═╝  ╚═══╝╚═╝  ╚═╝╚══════╝"#
}

fn parse_numeric_id(raw: &str) -> Result<String> {
    let s = raw.trim();
    anyhow::ensure!(!s.is_empty(), "Discord Application ID cannot be empty");
    anyhow::ensure!(
        s.chars().all(|c| c.is_ascii_digit()),
        "Discord Application ID must be numeric"
    );
    Ok(s.to_string())
}

fn default_agents() -> Vec<AgentRule> {
    vec![
        AgentRule {
            id: "claude_code".to_string(),
            label: Some("Claude Code".to_string()),
            process_names: vec!["claude".to_string(), "claude-code".to_string()],
            argv_substrings: vec![],
            large_image: Some("claude".to_string()),
            priority: 10,
            small_image: Some("devsignal".to_string()),
            small_text: Some("devsignal".to_string()),
            buttons: vec![ButtonConfig {
                label: "Claude Code Docs".to_string(),
                url: "https://claude.ai/code".to_string(),
            }],
        },
        AgentRule {
            id: "codex".to_string(),
            label: Some("Codex".to_string()),
            process_names: vec!["codex".to_string()],
            argv_substrings: vec![],
            large_image: Some("codex".to_string()),
            priority: 20,
            small_image: Some("devsignal".to_string()),
            small_text: Some("devsignal".to_string()),
            buttons: vec![ButtonConfig {
                label: "Codex on GitHub".to_string(),
                url: "https://github.com/openai/codex".to_string(),
            }],
        },
        AgentRule {
            id: "opencode".to_string(),
            label: Some("OpenCode".to_string()),
            process_names: vec!["opencode".to_string()],
            argv_substrings: vec![],
            large_image: Some("opencode".to_string()),
            priority: 30,
            small_image: Some("devsignal".to_string()),
            small_text: Some("devsignal".to_string()),
            buttons: vec![ButtonConfig {
                label: "OpenCode Docs".to_string(),
                url: "https://opencode.ai".to_string(),
            }],
        },
    ]
}

fn generate_config(
    discord_client_id: String,
    show_cwd_basename: bool,
    agents: Vec<AgentRule>,
) -> Config {
    Config {
        poll_interval_secs: 2,
        min_push_interval_secs: 20,
        idle_mode: IdleMode::Status,
        show_cwd_basename,
        discord: DiscordSection {
            client_id: discord_client_id,
            large_image: "devsignal".to_string(),
            large_text: "devsignal".to_string(),
            small_image: None,
            small_text: None,
        },
        agents,
    }
}

fn write_config_file(path: &Path, cfg: &Config, overwrite: bool) -> Result<()> {
    if path.exists() && !overwrite {
        anyhow::bail!(
            "config already exists at {} (refusing to overwrite without confirmation)",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config directory {}", parent.display()))?;
    }
    let toml = toml::to_string_pretty(cfg).context("serialize config to TOML")?;
    fs::write(path, toml).with_context(|| format!("write config {}", path.display()))?;
    Ok(())
}

fn choose_privacy_preset() -> Result<PrivacyPreset> {
    let items = vec![
        "Minimal (agent + host only)",
        "Project-safe (agent + host + project basename)",
        "Public/OSS (polished copy, no project names by default)",
        "Custom",
    ];
    let idx = Select::new()
        .with_prompt("Choose a privacy preset")
        .items(&items)
        .default(2)
        .interact()
        .context("read preset selection")?;

    Ok(match idx {
        0 => PrivacyPreset::Minimal,
        1 => PrivacyPreset::ProjectSafe,
        2 => PrivacyPreset::PublicOss,
        _ => PrivacyPreset::Custom,
    })
}

fn choose_agents() -> Result<Vec<AgentRule>> {
    let defaults = default_agents();
    let labels = vec!["Claude Code", "Codex", "OpenCode"];
    let selections = MultiSelect::new()
        .with_prompt("Select which agent rules to include")
        .items(&labels)
        .defaults(&[true, true, true])
        .interact()
        .context("read agent selection")?;

    let mut out = Vec::new();
    for idx in selections {
        if let Some(rule) = defaults.get(idx).cloned() {
            out.push(rule);
        }
    }
    Ok(out)
}

fn default_config_path_hint(path: &Path) -> String {
    format!("{}", style(path.display()).cyan())
}

fn repo_release_binary() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join("target").join("release").join("devsignal"))
        .filter(|p| p.exists())
}

fn expand_home(path: &str) -> Result<PathBuf> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").context("HOME is not set")?;
        return Ok(PathBuf::from(home).join(rest));
    }
    Ok(PathBuf::from(path))
}

fn current_uid() -> Result<String> {
    let out = Command::new("id").arg("-u").output().context("run id -u")?;
    anyhow::ensure!(out.status.success(), "id -u failed");
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn launch_agent_template() -> &'static str {
    include_str!("../../../packaging/macos/com.devsignal.daemon.example.plist")
}

fn generate_launch_agent_plist(bin_path: &Path, config_path: &Path) -> Result<String> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    let mut plist = launch_agent_template()
        .replace(
            "/REPLACE/WITH/ABSOLUTE/PATH/TO/devsignal",
            &bin_path.to_string_lossy(),
        )
        .replace("REPLACE_HOME", &home);

    // Replace ProgramArguments array to include `--config <path>`.
    let key = "<key>ProgramArguments</key>";
    let start = plist
        .find(key)
        .context("ProgramArguments key not found in plist template")?;
    let array_open = plist[start..]
        .find("<array>")
        .map(|i| start + i)
        .context("ProgramArguments <array> not found")?;
    let array_close = plist[array_open..]
        .find("</array>")
        .map(|i| array_open + i + "</array>".len())
        .context("ProgramArguments </array> not found")?;

    let mut new_array = String::new();
    new_array.push_str("<array>\n");
    new_array.push_str(&format!(
        "    <string>{}</string>\n",
        bin_path.to_string_lossy()
    ));
    new_array.push_str("    <string>--config</string>\n");
    new_array.push_str(&format!(
        "    <string>{}</string>\n",
        config_path.to_string_lossy()
    ));
    new_array.push_str("  </array>");

    plist.replace_range(array_open..array_close, &new_array);
    Ok(plist)
}

fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("create directory {}", path.display()))
}

fn copy_executable(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        ensure_dir(parent)?;
    }
    fs::copy(src, dst).with_context(|| format!("copy {} to {}", src.display(), dst.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(dst)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(dst, perms)?;
    }
    Ok(())
}

fn run_launchctl(args: &[&str]) -> Result<()> {
    let out = Command::new("launchctl")
        .args(args)
        .output()
        .with_context(|| format!("launchctl {}", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "launchctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

fn offer_full_local_setup(config_path: &Path) -> Result<()> {
    println!();
    println!("{}", style("Optional: local install and autostart").bold());

    let do_install = Confirm::new()
        .with_prompt("Install devsignal to ~/bin and set up LaunchAgent?")
        .default(true)
        .interact()
        .context("read install choice")?;
    if !do_install {
        return Ok(());
    }

    let bin_dst = expand_home("~/bin/devsignal")?;
    let logs_dir = expand_home("~/Library/Logs/devsignal")?;
    let plist_dst = expand_home("~/Library/LaunchAgents/com.devsignal.daemon.plist")?;

    if let Some(src) = repo_release_binary() {
        println!(
            "Found release binary at {}; copying to {}",
            style(src.display()).cyan(),
            style(bin_dst.display()).cyan()
        );
        copy_executable(&src, &bin_dst)?;
    } else {
        println!(
            "{}",
            style(
                "No target/release/devsignal found in current directory. Build it first with: cargo build --release -p devsignal-daemon"
            )
            .yellow()
        );
        let continue_anyway = Confirm::new()
            .with_prompt("Continue to set up logs + LaunchAgent anyway?")
            .default(true)
            .interact()
            .context("read continue anyway")?;
        if !continue_anyway {
            return Ok(());
        }
    }

    ensure_dir(&logs_dir)?;
    ensure_dir(
        plist_dst
            .parent()
            .context("LaunchAgents parent dir missing")?,
    )?;

    let plist = generate_launch_agent_plist(&bin_dst, config_path)?;
    fs::write(&plist_dst, plist)
        .with_context(|| format!("write LaunchAgent plist {}", plist_dst.display()))?;
    println!("Wrote LaunchAgent: {}", style(plist_dst.display()).cyan());

    let load = Confirm::new()
        .with_prompt("Load/Reload LaunchAgent now (launchctl bootstrap)?")
        .default(true)
        .interact()
        .context("read launchctl choice")?;
    if load {
        let uid = current_uid()?;
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/com.devsignal.daemon")])
            .output();
        run_launchctl(&[
            "bootstrap",
            &format!("gui/{uid}"),
            &plist_dst.to_string_lossy(),
        ])?;
        run_launchctl(&[
            "kickstart",
            "-k",
            &format!("gui/{uid}/com.devsignal.daemon"),
        ])?;
        println!("{}", style("LaunchAgent loaded.").green().bold());
    } else {
        println!("To load later:");
        println!("  launchctl bootstrap gui/$(id -u) {}", plist_dst.display());
    }

    Ok(())
}

pub fn cmd_init(config_path: &Path) -> Result<()> {
    println!("{}", style(banner()).cyan());
    println!();
    println!(
        "{}",
        style("Welcome to devsignal init — a guided setup wizard.").bold()
    );
    println!("This will write a config file and help you validate Rich Presence on this machine.");
    println!();
    println!(
        "Target config path: {}",
        default_config_path_hint(config_path)
    );
    println!();

    let discord_client_id: String = Input::new()
        .with_prompt("Discord Application ID (numeric)")
        .validate_with(|s: &String| parse_numeric_id(s).map(|_| ()))
        .interact_text()
        .context("read Discord Application ID")?;
    let discord_client_id = parse_numeric_id(&discord_client_id)?;

    let preset = choose_privacy_preset()?;
    let show_cwd_basename = match preset {
        PrivacyPreset::Minimal | PrivacyPreset::PublicOss => false,
        PrivacyPreset::ProjectSafe => true,
        PrivacyPreset::Custom => Confirm::new()
            .with_prompt("Show project basename (CWD leaf) in Discord?")
            .default(false)
            .interact()
            .context("read show_cwd_basename")?,
    };

    let agents = choose_agents()?;
    anyhow::ensure!(
        !agents.is_empty(),
        "at least one agent must be selected (Config requires [[agents]])"
    );

    println!();
    println!("{}", style("Art assets (optional for now):").bold());
    println!("If you want images later, upload keys under Discord Developer Portal → Rich Presence → Art Assets:");
    println!("  - devsignal");
    println!("  - claude");
    println!("  - codex");
    println!("  - opencode");
    println!();

    let overwrite = if config_path.exists() {
        Confirm::new()
            .with_prompt(format!(
                "Config already exists at {} — overwrite?",
                config_path.display()
            ))
            .default(false)
            .interact()
            .context("read overwrite confirmation")?
    } else {
        false
    };

    let cfg = generate_config(discord_client_id, show_cwd_basename, agents);
    write_config_file(config_path, &cfg, overwrite)?;

    // Validate by re-loading from disk (uses core validation).
    let _ = Config::load_from_path(config_path).context("validate written config")?;

    println!();
    println!("{}", style("Config written and validated.").green().bold());
    println!("Next steps:");
    println!("  - Validate: {}", style("devsignal validate").cyan());
    println!("  - Dry-run:  {}", style("devsignal once").cyan());
    println!("  - Run:      {}", style("devsignal run").cyan());
    offer_full_local_setup(config_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numeric_id_rejects_non_digits() {
        assert!(parse_numeric_id("abc").is_err());
        assert!(parse_numeric_id("123a").is_err());
        assert!(parse_numeric_id("").is_err());
        assert_eq!(parse_numeric_id("123").unwrap(), "123");
    }

    #[test]
    fn generate_config_sets_cwd_flag() {
        let cfg = generate_config("1".into(), true, default_agents());
        assert!(cfg.show_cwd_basename);
        assert_eq!(cfg.discord.client_id, "1");
        assert!(!cfg.agents.is_empty());
    }
}
