//! Centralized path management for application data directories.
//!
//! All paths derive from a single data root, resolved once via `OnceLock`.
//! The root can be overridden by setting the `APP_DATA_DIR` environment variable.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Root data directory, resolved in order:
/// 1. `APP_DATA_DIR` env var (must be non-empty and an absolute path)
/// 2. `~/.rara-trading`
///
/// # Panics
/// Panics if `APP_DATA_DIR` is set but empty or not an absolute path,
/// or if no home directory can be resolved and `APP_DATA_DIR` is unset.
pub fn data_dir() -> &'static Path {
    DATA_DIR.get_or_init(|| {
        if let Ok(dir) = std::env::var("APP_DATA_DIR") {
            let path = PathBuf::from(&dir);
            assert!(
                !dir.is_empty() && path.is_absolute(),
                "APP_DATA_DIR must be a non-empty absolute path, got: {dir:?}"
            );
            return path;
        }

        dirs::home_dir()
            .expect("home directory must be resolvable — set APP_DATA_DIR as a fallback")
            .join(".rara-trading")
    })
}

/// Config file path: `<data>/config.toml`
pub fn config_file() -> PathBuf { data_dir().join("config.toml") }

/// Cache directory: `<data>/cache`
pub fn cache_dir() -> PathBuf { data_dir().join("cache") }

/// Strategy generated code directory: `<data>/strategies/generated`
pub fn strategies_generated_dir() -> PathBuf { data_dir().join("strategies/generated") }

/// Strategy promoted directory: `<data>/strategies/promoted`
pub fn strategies_promoted_dir() -> PathBuf { data_dir().join("strategies/promoted") }

/// Path to the accounts configuration file.
pub fn accounts_file() -> PathBuf { data_dir().join("accounts.toml") }
