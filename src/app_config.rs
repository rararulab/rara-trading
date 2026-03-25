//! Application configuration backed by TOML file.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::agent::AgentConfig;

static APP_CONFIG: OnceLock<AppConfig> = OnceLock::new();

/// Application configuration.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Agent backend configuration.
    pub agent: AgentConfig,
}

/// Load config from TOML file, falling back to defaults.
///
/// The result is cached in a `OnceLock` — subsequent calls return the same
/// value even after [`save`]. This is fine for CLI usage (one command per
/// process) but callers using this as a library should be aware of the
/// caching behavior.
pub fn load() -> &'static AppConfig {
    APP_CONFIG.get_or_init(|| {
        let path = crate::paths::config_file();
        if path.exists() {
            let settings = config::Config::builder()
                .add_source(config::File::from(path.as_ref()))
                .build()
                .unwrap_or_default();
            settings.try_deserialize().unwrap_or_default()
        } else {
            AppConfig::default()
        }
    })
}

/// Save config to TOML file.
pub fn save(cfg: &AppConfig) -> std::io::Result<()> {
    let path = crate::paths::config_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(cfg).expect("config serialization should not fail");
    std::fs::write(path, content)
}
