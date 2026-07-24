//! Writing config files back to disk.
//!
//! Both the `init` wizard and the `hosts`/`agents`/`rules` subcommands rewrite the user's config.
//! Doing that with a plain `fs::write` risks truncating a good config if the process dies mid-write,
//! and validating afterwards reports the problem only once the original is already gone. So the
//! order here is: serialize, round-trip, validate, write a sibling temp file, then rename.

use anyhow::{Context, Result};
use devsignal_core::Config;
use std::fs;
use std::path::Path;

/// Serialize and validate `cfg`, then replace `path` atomically.
///
/// Note for callers: TOML comments and key ordering do not survive this, because the config is
/// rewritten from the deserialized struct. That is why `hosts`/`agents`/`rules` warn the user.
pub fn write_config_atomic(path: &Path, cfg: &Config) -> Result<()> {
    let toml = toml::to_string_pretty(cfg).context("serialize config to TOML")?;

    // Round-trip before touching the filesystem: this catches a serialize/deserialize mismatch
    // while the user's existing file is still intact.
    let round: Config =
        toml::from_str(&toml).context("serialized config did not parse back (internal bug)")?;
    round
        .validate()
        .context("refusing to write a config that would not load")?;

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("create config directory {}", parent.display()))?;

    // The temp file must be a sibling: `rename` is only atomic within one filesystem.
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("config path has no file name")?;
    let tmp = parent.join(format!(".{file_name}.tmp"));

    fs::write(&tmp, toml.as_bytes())
        .with_context(|| format!("write temporary config {}", tmp.display()))?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("replace config {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use devsignal_core::{AgentRule, ButtonConfig};

    fn cfg_with_client_id(id: &str) -> Config {
        let toml = format!(
            r#"
            [discord]
            client_id = "{id}"

            [[agents]]
            id = "claude_code"
            process_names = ["claude"]
            "#
        );
        toml::from_str(&toml).expect("fixture parses")
    }

    #[test]
    fn writes_and_reloads_a_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let cfg = cfg_with_client_id("123456789");

        write_config_atomic(&path, &cfg).expect("write");

        let reloaded = Config::load_from_path(&path).expect("reload");
        assert_eq!(reloaded.discord.client_id, "123456789");
        assert_eq!(reloaded.agents.len(), 1);
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/deeper/config.toml");
        write_config_atomic(&path, &cfg_with_client_id("42")).expect("write");
        assert!(path.exists());
    }

    #[test]
    fn leaves_the_original_intact_when_the_new_config_is_invalid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        write_config_atomic(&path, &cfg_with_client_id("111")).expect("first write");
        let before = fs::read_to_string(&path).expect("read");

        // Three buttons is rejected by Config::validate.
        let mut bad = cfg_with_client_id("111");
        bad.agents[0].buttons = (0..3)
            .map(|i| ButtonConfig {
                label: format!("b{i}"),
                url: "https://example.com".into(),
            })
            .collect();

        let err = write_config_atomic(&path, &bad).expect_err("should refuse");
        assert!(format!("{err:#}").contains("would not load"));
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            before,
            "the existing config must be untouched"
        );
    }

    #[test]
    fn does_not_leave_temp_files_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        write_config_atomic(&path, &cfg_with_client_id("7")).expect("write");

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "found temp files: {leftovers:?}");
    }

    #[test]
    fn overwrites_an_existing_config_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        write_config_atomic(&path, &cfg_with_client_id("111")).expect("first");

        let mut updated = cfg_with_client_id("222");
        updated.agents.push(AgentRule {
            id: "codex".into(),
            label: None,
            process_names: vec!["codex".into()],
            argv_substrings: vec![],
            large_image: None,
            priority: 20,
            small_image: None,
            small_text: None,
            buttons: vec![],
        });
        write_config_atomic(&path, &updated).expect("second");

        let reloaded = Config::load_from_path(&path).expect("reload");
        assert_eq!(reloaded.discord.client_id, "222");
        assert_eq!(reloaded.agents.len(), 2);
    }
}
