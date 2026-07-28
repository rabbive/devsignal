use anyhow::{Context, Result};
use console::style;
use devsignal_core::{
    agent_presets, host_image_keys, parse_numeric_id, AgentRule, Config, DiscordSection, IdleMode,
    ImageMode, ImagesConfig, PlatformsConfig, PresenceLine, PresenceRule, PresenceSection,
    RuleThen, RuleWhen, TimeWindow, HOST_APPS,
};
use dialoguer::{Confirm, Input, MultiSelect, Select};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// How much detail reaches Discord. These must differ in behaviour, not just in wording:
/// `Minimal` hides the host via a catch-all rule, `Balanced` shows the host, `Detailed` adds the
/// project directory name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrivacyPreset {
    Minimal,
    Balanced,
    Detailed,
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

/// The shipped preset table lives in `devsignal-core` so the wizard and `config.example.toml`
/// cannot drift apart.
fn default_agents() -> Vec<AgentRule> {
    agent_presets()
}

fn generate_config(
    discord_client_id: String,
    show_cwd_basename: bool,
    presence: PresenceSection,
    images: ImagesConfig,
    agents: Vec<AgentRule>,
    disabled_hosts: Vec<String>,
    rules: Vec<PresenceRule>,
) -> Config {
    Config {
        poll_interval_secs: 2,
        min_push_interval_secs: 20,
        idle_mode: IdleMode::Status,
        show_cwd_basename,
        presence,
        images,
        discord: DiscordSection {
            client_id: discord_client_id,
            large_image: "devsignal".to_string(),
            large_text: "devsignal".to_string(),
            small_image: None,
            small_text: None,
        },
        agents,
        platforms: PlatformsConfig {
            disabled_hosts,
            disabled_agents: vec![],
        },
        rules,
    }
}

fn write_config_file(path: &Path, cfg: &Config, overwrite: bool) -> Result<()> {
    if path.exists() && !overwrite {
        anyhow::bail!(
            "config already exists at {} (refusing to overwrite without confirmation)",
            path.display()
        );
    }
    crate::config_io::write_config_atomic(path, cfg)
}

fn choose_privacy_preset() -> Result<PrivacyPreset> {
    let items = vec![
        "Minimal — agent only (host hidden)",
        "Balanced — agent + editor/terminal",
        "Detailed — agent + editor/terminal + project folder name",
        "Custom",
    ];
    let idx = Select::new()
        .with_prompt("How much should Discord show?")
        .items(&items)
        .default(1)
        .interact()
        .context("read preset selection")?;

    Ok(match idx {
        0 => PrivacyPreset::Minimal,
        1 => PrivacyPreset::Balanced,
        2 => PrivacyPreset::Detailed,
        _ => PrivacyPreset::Custom,
    })
}

/// `Minimal` needs a catch-all rule to suppress the host label. It goes *last* so any rule the user
/// picked in `choose_rule_presets` still wins under first-match-wins.
fn privacy_preset_rules(preset: PrivacyPreset) -> Vec<PresenceRule> {
    if preset != PrivacyPreset::Minimal {
        return vec![];
    }
    vec![PresenceRule {
        name: "minimal_hide_host".into(),
        when: RuleWhen::default(),
        then: RuleThen {
            hide_host: true,
            state: None,
        },
    }]
}

fn choose_agents() -> Result<Vec<AgentRule>> {
    let defaults = default_agents();
    let labels = defaults
        .iter()
        .map(|a| {
            let name = a.label.clone().unwrap_or_else(|| a.id.clone());
            format!("{name} ({})", a.process_names.join(", "))
        })
        .collect::<Vec<_>>();
    let all = vec![true; labels.len()];
    let selections = MultiSelect::new()
        .with_prompt(
            "Select which agent CLIs to watch (space toggles; a rule only matters when that CLI runs)",
        )
        .items(&labels)
        .defaults(&all)
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

/// Which of Discord's three lines carries what.
///
/// The classic layout is offered first, and is the default, because line 1 is the Discord
/// *application's* name unless the client honours the activity-name override. Where it does not,
/// "agent first" costs you the agent label entirely — so this is an opt-in with the trade-off
/// spelled out, not a silent new default.
fn choose_presence_layout(show_cwd_basename: bool) -> Result<PresenceSection> {
    let items = vec![
        "Classic — devsignal / Claude Code / In Ghostty",
        "Agent first — Claude Code / In Ghostty / devsignal (overrides the app name on line 1)",
    ];
    let idx = Select::new()
        .with_prompt("Line order in the Discord card")
        .items(&items)
        .default(0)
        .interact()
        .context("read presence layout selection")?;

    if idx == 0 {
        return Ok(PresenceSection::default());
    }
    Ok(PresenceSection {
        name: PresenceLine::Agent,
        details: PresenceLine::Host,
        // With the project on its own line it stops riding along as "In Ghostty · myrepo".
        state: if show_cwd_basename {
            PresenceLine::Project
        } else {
            PresenceLine::Brand
        },
        brand_text: "devsignal".to_string(),
    })
}

/// Hosted PNGs or uploaded art-asset keys.
///
/// `host_icon` is only offered in url mode: in key mode it would need every icon in
/// [`host_image_keys`] uploaded by hand, and a missing key shows as a blank circle.
fn choose_images() -> Result<ImagesConfig> {
    let items = vec![
        "Hosted PNGs — nothing to upload (assets/discord/ on GitHub)",
        "Art-asset keys — images you upload in the Discord Developer Portal",
    ];
    let idx = Select::new()
        .with_prompt("Where should presence images come from?")
        .items(&items)
        .default(0)
        .interact()
        .context("read image mode selection")?;

    if idx == 1 {
        return Ok(ImagesConfig {
            mode: ImageMode::Key,
            host_icon: false,
            ..ImagesConfig::default()
        });
    }

    let host_icon = Confirm::new()
        .with_prompt("Show the frontmost editor/terminal as the small corner icon?")
        .default(true)
        .interact()
        .context("read host_icon")?;
    Ok(ImagesConfig {
        mode: ImageMode::Url,
        host_icon,
        ..ImagesConfig::default()
    })
}

fn choose_disabled_hosts() -> Result<Vec<String>> {
    let labels = HOST_APPS
        .iter()
        .map(|app| format!("{} ({})", app.label, app.bundle_id))
        .collect::<Vec<_>>();
    let defaults = vec![true; labels.len()];
    let selections = MultiSelect::new()
        .with_prompt("Select host platforms DevSignal may show (all enabled by default)")
        .items(&labels)
        .defaults(&defaults)
        .interact()
        .context("read host selection")?;
    let enabled = selections
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    Ok(HOST_APPS
        .iter()
        .enumerate()
        .filter(|(idx, _)| !enabled.contains(idx))
        .map(|(_, app)| app.bundle_id.to_string())
        .collect())
}

fn choose_rule_presets() -> Result<Vec<PresenceRule>> {
    let items = vec![
        "Focus mode in Terminal (Claude Code -> Deep work, hide host)",
        "After-hours privacy (22:00-06:00 -> Heads down, hide host)",
        "Hide host for secret projects (basename: secret -> Private build)",
    ];
    let selected = MultiSelect::new()
        .with_prompt("Optional creative presence rules")
        .items(&items)
        .defaults(&[false, false, false])
        .interact()
        .context("read rule presets")?;

    let mut rules = Vec::new();
    for idx in selected {
        match idx {
            0 => rules.push(PresenceRule {
                name: "terminal_deep_work".into(),
                when: RuleWhen {
                    host_bundle_ids: vec![
                        "com.apple.Terminal".into(),
                        "com.googlecode.iterm2".into(),
                    ],
                    agent_ids: vec!["claude_code".into()],
                    active_only: true,
                    idle_only: false,
                    project_basenames: vec![],
                    time: None,
                },
                then: RuleThen {
                    hide_host: true,
                    state: Some("Deep work".into()),
                },
            }),
            1 => rules.push(PresenceRule {
                name: "after_hours_privacy".into(),
                when: RuleWhen {
                    host_bundle_ids: vec![],
                    agent_ids: vec![],
                    active_only: true,
                    idle_only: false,
                    project_basenames: vec![],
                    time: Some(TimeWindow {
                        start: "22:00".into(),
                        end: "06:00".into(),
                    }),
                },
                then: RuleThen {
                    hide_host: true,
                    state: Some("Heads down".into()),
                },
            }),
            2 => rules.push(PresenceRule {
                name: "secret_project_privacy".into(),
                when: RuleWhen {
                    host_bundle_ids: vec![],
                    agent_ids: vec![],
                    active_only: true,
                    idle_only: false,
                    project_basenames: vec!["secret".into()],
                    time: None,
                },
                then: RuleThen {
                    hide_host: true,
                    state: Some("Private build".into()),
                },
            }),
            _ => {}
        }
    }
    Ok(rules)
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

/// Escape a path for interpolation into a plist `<string>`. A home or config path containing `&`
/// or `<` would otherwise produce a malformed plist that `launchctl bootstrap` rejects with an
/// opaque error.
fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn generate_launch_agent_plist(bin_path: &Path, config_path: &Path) -> Result<String> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    let bin = xml_escape(&bin_path.to_string_lossy());
    let config = xml_escape(&config_path.to_string_lossy());

    let mut plist = launch_agent_template()
        .replace("/REPLACE/WITH/ABSOLUTE/PATH/TO/devsignal", &bin)
        .replace("REPLACE_HOME", &xml_escape(&home));

    // Rewrite the ProgramArguments array to add `--config <path>`: launchd runs with a minimal
    // environment, so the daemon must not rely on resolving the default path itself.
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

    let new_array = format!(
        "<array>\n    \
         <string>{bin}</string>\n    \
         <string>--config</string>\n    \
         <string>{config}</string>\n  \
         </array>"
    );

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
        PrivacyPreset::Minimal | PrivacyPreset::Balanced => false,
        PrivacyPreset::Detailed => true,
        PrivacyPreset::Custom => Confirm::new()
            .with_prompt("Show project basename (CWD leaf) in Discord?")
            .default(false)
            .interact()
            .context("read show_cwd_basename")?,
    };

    let presence = choose_presence_layout(show_cwd_basename)?;
    let images = choose_images()?;

    let agents = choose_agents()?;
    anyhow::ensure!(
        !agents.is_empty(),
        "at least one agent must be selected (Config requires [[agents]])"
    );
    let disabled_hosts = choose_disabled_hosts()?;
    // User-chosen rules first, then any rule the privacy preset implies, so the explicit choice
    // wins under first-match-wins.
    let mut rules = choose_rule_presets()?;
    rules.extend(privacy_preset_rules(preset));

    println!();
    println!("{}", style("Art assets").bold());
    let mut keys = vec!["devsignal".to_string()];
    for agent in &agents {
        if let Some(key) = &agent.large_image {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }
    match images.mode {
        ImageMode::Url => {
            println!(
                "Nothing to upload: images resolve to PNGs under\n  {}\n\
                 Discord accepts a plain https image URL wherever it accepts an asset key.",
                images.base_url
            );
            println!("Agent images your selection uses:");
            for key in &keys {
                println!("  - {}/agents/{key}.png", images.base_url);
            }
        }
        ImageMode::Key => {
            println!(
                "Presence images are Discord asset KEYS, uploaded under\n\
                 Developer Portal → Rich Presence → Art Assets. A key you have not uploaded renders \
                 blank —\nremove the large_image line for that agent to fall back to \"devsignal\". \
                 The PNGs to\nupload are in assets/discord/ in the devsignal repo."
            );
            println!("Keys referenced by your selection:");
            for key in &keys {
                println!("  - {key}");
            }
            println!(
                "Host icons are off in key mode; turning on images.host_icon means uploading\n\
                 these {} host keys too: {}",
                host_image_keys().len(),
                host_image_keys().join(", ")
            );
        }
    }
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

    let cfg = generate_config(
        discord_client_id,
        show_cwd_basename,
        presence,
        images,
        agents,
        disabled_hosts,
        rules,
    );
    write_config_file(config_path, &cfg, overwrite)?;
    // write_config_atomic validates before replacing the file, so reaching here means it loads.

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
    fn generate_config_sets_cwd_flag() {
        let cfg = generate_config(
            "1".into(),
            true,
            PresenceSection::default(),
            ImagesConfig::default(),
            default_agents(),
            vec![],
            vec![],
        );
        assert!(cfg.show_cwd_basename);
        assert_eq!(cfg.discord.client_id, "1");
        assert!(!cfg.agents.is_empty());
    }

    #[test]
    fn wizard_defaults_come_from_the_core_preset_table() {
        let wizard: Vec<String> = default_agents().into_iter().map(|a| a.id).collect();
        let core: Vec<String> = agent_presets().into_iter().map(|a| a.id).collect();
        assert_eq!(wizard, core);
    }

    #[test]
    fn generated_config_from_presets_is_valid() {
        let cfg = generate_config(
            "123456789".into(),
            false,
            PresenceSection::default(),
            ImagesConfig::default(),
            default_agents(),
            vec![],
            vec![],
        );
        cfg.validate().expect("wizard output must load");
    }

    /// Regression: the old `PublicOss` preset was byte-identical in behaviour to `Minimal`.
    #[test]
    fn each_privacy_preset_produces_a_distinct_config() {
        let shape = |preset: PrivacyPreset, show_cwd: bool| {
            let cfg = generate_config(
                "123456789".into(),
                show_cwd,
                PresenceSection::default(),
                ImagesConfig::default(),
                default_agents(),
                vec![],
                privacy_preset_rules(preset),
            );
            cfg.validate().expect("preset output must load");
            (cfg.show_cwd_basename, cfg.rules.len())
        };

        let minimal = shape(PrivacyPreset::Minimal, false);
        let balanced = shape(PrivacyPreset::Balanced, false);
        let detailed = shape(PrivacyPreset::Detailed, true);

        assert_ne!(minimal, balanced, "Minimal and Balanced must differ");
        assert_ne!(balanced, detailed, "Balanced and Detailed must differ");
        assert_ne!(minimal, detailed, "Minimal and Detailed must differ");

        // Minimal is the only one that needs a rule, and it hides the host.
        assert_eq!(minimal.1, 1);
        assert_eq!(balanced.1, 0);
        assert_eq!(detailed.1, 0);
    }

    #[test]
    fn minimal_privacy_rule_hides_host_and_is_a_catch_all() {
        let rules = privacy_preset_rules(PrivacyPreset::Minimal);
        assert_eq!(rules.len(), 1);
        assert!(rules[0].then.hide_host);
        assert!(rules[0].then.state.is_none());
        // A catch-all `when` is what makes it a fallback for every situation.
        assert!(rules[0].when.host_bundle_ids.is_empty());
        assert!(rules[0].when.agent_ids.is_empty());
        assert!(!rules[0].when.active_only);
        assert!(!rules[0].when.idle_only);

        for other in [PrivacyPreset::Balanced, PrivacyPreset::Detailed] {
            assert!(privacy_preset_rules(other).is_empty());
        }
    }

    /// The template must stay well-formed XML. It previously contained `--` inside an XML comment
    /// (from `cargo build --release`), which is illegal and rejected by any strict parser.
    #[test]
    fn launch_agent_template_has_no_double_hyphen_in_comments() {
        let tpl = launch_agent_template();
        let start = tpl.find("<!--").expect("template has a comment");
        let end = tpl.find("-->").expect("comment is closed");
        let body = &tpl[start + 4..end];
        assert!(
            !body.contains("--"),
            "XML comments must not contain a double hyphen"
        );
    }

    #[test]
    fn generated_plist_contains_binary_and_config_paths() {
        std::env::set_var("HOME", "/Users/demo");
        let plist = generate_launch_agent_plist(
            Path::new("/Users/demo/bin/devsignal"),
            Path::new("/Users/demo/.config/devsignal/config.toml"),
        )
        .expect("generate");

        assert!(plist.contains("<string>/Users/demo/bin/devsignal</string>"));
        assert!(plist.contains("<string>--config</string>"));
        assert!(
            plist.contains("<string>/Users/demo/.config/devsignal/config.toml</string>"),
            "config path must be passed explicitly"
        );
        // The placeholders must all be substituted.
        assert!(!plist.contains("REPLACE_HOME"), "REPLACE_HOME left behind");
        assert!(
            !plist.contains("/REPLACE/WITH"),
            "binary placeholder left behind"
        );
        assert!(plist.contains("/Users/demo/Library/Logs/devsignal/devsignal.out.log"));
        // Exactly one array, and tags stay balanced after the surgery.
        assert_eq!(plist.matches("<array>").count(), 1);
        assert_eq!(plist.matches("</array>").count(), 1);
        assert_eq!(plist.matches("<plist").count(), 1);
        assert_eq!(plist.matches("</plist>").count(), 1);
    }

    /// `KeepAlive` restarts the daemon on any nonzero exit, and exit codes cannot tell launchd that a
    /// failure is permanent — a Rust panic exits 101 like anything else. Without this key launchd
    /// respawns at its 10-second default, so a daemon that cannot start burns a process every 10s
    /// indefinitely.
    #[test]
    fn generated_plist_throttles_respawns() {
        std::env::set_var("HOME", "/Users/demo");
        let plist = generate_launch_agent_plist(
            Path::new("/Users/demo/bin/devsignal"),
            Path::new("/Users/demo/.config/devsignal/config.toml"),
        )
        .expect("generate");

        assert!(
            plist.contains("<key>ThrottleInterval</key>"),
            "the wizard's plist must throttle respawns"
        );
        assert!(
            plist.contains("<integer>60</integer>"),
            "ThrottleInterval should be 60 seconds"
        );
        // Quiet by default: the launchd log has no rotation, and failures already log on transition
        // rather than per retry.
        assert!(
            plist.contains("<string>warn</string>"),
            "RUST_LOG should default to warn under launchd"
        );
    }

    #[test]
    fn generated_plist_escapes_xml_significant_characters() {
        std::env::set_var("HOME", "/Users/a&b");
        let plist = generate_launch_agent_plist(
            Path::new("/Users/a&b/bin/devsignal"),
            Path::new("/Users/a&b/cfg<1>.toml"),
        )
        .expect("generate");

        assert!(plist.contains("/Users/a&amp;b/bin/devsignal"));
        assert!(plist.contains("cfg&lt;1&gt;.toml"));
        // No bare ampersand may survive: every & must begin an entity.
        for (idx, _) in plist.match_indices('&') {
            let tail = &plist[idx..];
            assert!(
                tail.starts_with("&amp;") || tail.starts_with("&lt;") || tail.starts_with("&gt;"),
                "bare & at byte {idx} would make the plist malformed"
            );
        }
    }

    #[test]
    fn xml_escape_handles_each_significant_character() {
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(xml_escape("plain/path"), "plain/path");
        // Ampersand is escaped first, so an escape is not double-escaped.
        assert_eq!(xml_escape("&lt;"), "&amp;lt;");
    }

    /// The privacy fallback must not shadow a rule the user explicitly chose.
    #[test]
    fn privacy_rule_is_appended_after_user_rules() {
        let mut rules = vec![PresenceRule {
            name: "user_rule".into(),
            when: RuleWhen::default(),
            then: RuleThen {
                hide_host: false,
                state: Some("Streaming".into()),
            },
        }];
        rules.extend(privacy_preset_rules(PrivacyPreset::Minimal));
        assert_eq!(rules[0].name, "user_rule");
        assert_eq!(rules[1].name, "minimal_hide_host");
    }
}
