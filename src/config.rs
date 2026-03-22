use anyhow::{Context, Result};
use std::path::PathBuf;

/// Returns the path to the sigiro config directory (~/.config/sigiro/).
fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("cannot determine config directory")?;
    Ok(base.join("sigiro"))
}

/// Returns the path to config.toml.
fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Resolve API key: env var takes priority, then config.toml.
pub fn get_api_key() -> Result<String> {
    if let Ok(key) = std::env::var("SIGIRO_API_KEY") {
        return Ok(key);
    }
    let path = config_path()?;
    let content = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "No API key found. Run: sigiro auth <key>\n  (looked in {} and $SIGIRO_API_KEY)",
            path.display()
        )
    })?;
    let config: toml::Value = content.parse().context("invalid config.toml")?;
    config["api_key"]
        .as_str()
        .map(String::from)
        .context("api_key not found in config.toml")
}

/// Save API key to ~/.config/sigiro/config.toml with 0600 permissions on Unix.
pub fn save_api_key(key: &str) -> Result<PathBuf> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create config dir: {}", dir.display()))?;
    let path = dir.join("config.toml");
    std::fs::write(&path, format!("api_key = \"{key}\"\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .context("failed to set config.toml permissions to 0600")?;
    }
    Ok(path)
}
