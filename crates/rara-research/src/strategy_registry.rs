//! Strategy registry — fetch pre-built WASM strategies from GitHub Releases.
//!
//! Connects to the `rararulab/rara-strategies` GitHub repository, lists
//! available releases, downloads WASM artifacts, validates API version
//! compatibility, and saves them to the promoted strategies directory.

use std::path::PathBuf;

use bon::Builder;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use tracing::{debug, info};

use rara_strategy_api::API_VERSION;

use crate::strategy_executor::StrategyExecutor;
use crate::wasm_executor::WasmExecutor;

/// Errors from strategy registry operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum RegistryError {
    /// HTTP request to GitHub API failed.
    #[snafu(display("GitHub API request failed: {source}"))]
    Http {
        /// The underlying reqwest error.
        source: reqwest::Error,
    },

    /// GitHub API returned a non-success status.
    #[snafu(display("GitHub API error {status}: {body}"))]
    GitHubApi {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },

    /// No WASM asset found in the release.
    #[snafu(display("no .wasm asset found in release {tag}"))]
    NoWasmAsset {
        /// The release tag name.
        tag: String,
    },

    /// WASM module API version is incompatible.
    #[snafu(display(
        "API version mismatch: strategy requires v{strategy_version}, runtime supports v{runtime_version}"
    ))]
    ApiVersionMismatch {
        /// The strategy's API version.
        strategy_version: u32,
        /// The runtime's supported API version.
        runtime_version: u32,
    },

    /// WASM runtime validation failed.
    #[snafu(display("WASM validation failed: {source}"))]
    WasmValidation {
        /// The underlying executor error.
        source: crate::strategy_executor::ExecutorError,
    },

    /// Filesystem I/O failed.
    #[snafu(display("I/O error: {source}"))]
    Io {
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// JSON serialization/deserialization failed.
    #[snafu(display("JSON error: {source}"))]
    Json {
        /// The underlying serde error.
        source: serde_json::Error,
    },
}

/// Module-level result alias.
pub type Result<T> = std::result::Result<T, RegistryError>;

/// A release entry from the GitHub strategy registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Release tag name (e.g. "btc-momentum-v0.1.0").
    pub tag: String,
    /// Strategy name derived from the tag.
    pub name: String,
    /// Version string derived from the tag.
    pub version: String,
    /// WASM asset download URL.
    pub wasm_url: String,
    /// WASM asset filename.
    pub wasm_filename: String,
    /// Asset size in bytes.
    pub size: u64,
}

/// Metadata saved alongside a fetched registry strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedStrategy {
    /// Original registry entry.
    pub entry: RegistryEntry,
    /// Strategy metadata extracted from the WASM module.
    pub meta: rara_strategy_api::StrategyMeta,
    /// Local filesystem path to the saved WASM binary.
    pub wasm_path: PathBuf,
}

/// GitHub Release asset from the API response.
#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// GitHub Release from the API response.
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

/// Client for the rara-strategies GitHub Release registry.
#[derive(Debug, Builder)]
pub struct StrategyRegistry {
    /// GitHub repository in "owner/repo" format.
    #[builder(default = "rararulab/rara-strategies".to_string())]
    repo: String,

    /// Directory to save fetched WASM artifacts.
    promoted_dir: PathBuf,
}

impl StrategyRegistry {
    /// List all available strategies from the GitHub registry.
    pub async fn list_available(&self) -> Result<Vec<RegistryEntry>> {
        let url = format!(
            "https://api.github.com/repos/{}/releases",
            self.repo
        );

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .header("User-Agent", "rara-trading")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .context(HttpSnafu)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::GitHubApi {
                status: status.as_u16(),
                body,
            });
        }

        let releases: Vec<GitHubRelease> = response.json().await.context(HttpSnafu)?;

        let entries = releases
            .into_iter()
            .filter_map(|release| {
                let wasm_asset = release
                    .assets
                    .into_iter()
                    .find(|a| {
                        std::path::Path::new(&a.name)
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("wasm"))
                    })?;

                let (name, version) = parse_tag(&release.tag_name)?;

                Some(RegistryEntry {
                    tag: release.tag_name,
                    name,
                    version,
                    wasm_url: wasm_asset.browser_download_url,
                    wasm_filename: wasm_asset.name,
                    size: wasm_asset.size,
                })
            })
            .collect();

        Ok(entries)
    }

    /// Fetch a strategy by name, validate it, and save to the promoted directory.
    pub async fn fetch(&self, strategy_name: &str) -> Result<FetchedStrategy> {
        let entries = self.list_available().await?;

        let entry = entries
            .into_iter()
            .find(|e| e.name == strategy_name)
            .ok_or_else(|| RegistryError::NoWasmAsset {
                tag: strategy_name.to_string(),
            })?;

        self.fetch_entry(&entry).await
    }

    /// Fetch a specific registry entry, validate, and save.
    async fn fetch_entry(&self, entry: &RegistryEntry) -> Result<FetchedStrategy> {
        info!(strategy = %entry.name, version = %entry.version, "fetching strategy from registry");

        // Download the WASM binary
        let client = reqwest::Client::new();
        let wasm_bytes = client
            .get(&entry.wasm_url)
            .header("User-Agent", "rara-trading")
            .send()
            .await
            .context(HttpSnafu)?
            .bytes()
            .await
            .context(HttpSnafu)?;

        debug!(
            strategy = %entry.name,
            size = wasm_bytes.len(),
            "downloaded WASM artifact"
        );

        // Validate: load into WASM runtime and extract metadata
        let executor = WasmExecutor::builder().build();
        let mut handle = executor.load(&wasm_bytes).context(WasmValidationSnafu)?;
        let meta = handle.meta().context(WasmValidationSnafu)?;

        // Check API version compatibility
        if meta.api_version != API_VERSION {
            return Err(RegistryError::ApiVersionMismatch {
                strategy_version: meta.api_version,
                runtime_version: API_VERSION,
            });
        }

        info!(
            strategy = %meta.name,
            version = meta.version,
            api_version = meta.api_version,
            "WASM validation passed"
        );

        // Save to promoted directory
        std::fs::create_dir_all(&self.promoted_dir).context(IoSnafu)?;

        let wasm_path = self
            .promoted_dir
            .join(format!("{}.wasm", entry.name));
        std::fs::write(&wasm_path, &wasm_bytes).context(IoSnafu)?;

        // Save metadata JSON alongside the WASM file
        let meta_path = self
            .promoted_dir
            .join(format!("{}.registry.json", entry.name));
        let fetched = FetchedStrategy {
            entry: entry.clone(),
            meta,
            wasm_path: wasm_path.clone(),
        };
        let json = serde_json::to_string_pretty(&fetched).context(JsonSnafu)?;
        std::fs::write(&meta_path, json).context(IoSnafu)?;

        info!(
            strategy = %entry.name,
            path = %wasm_path.display(),
            "strategy saved to promoted directory"
        );

        Ok(fetched)
    }

    /// List strategies that have been fetched from the registry and saved locally.
    pub fn list_installed(&self) -> Result<Vec<FetchedStrategy>> {
        if !self.promoted_dir.exists() {
            return Ok(Vec::new());
        }

        let mut installed = Vec::new();
        let entries = std::fs::read_dir(&self.promoted_dir).context(IoSnafu)?;

        for entry in entries {
            let entry = entry.context(IoSnafu)?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".registry.json"))
            {
                let content = std::fs::read_to_string(&path).context(IoSnafu)?;
                let strategy: FetchedStrategy =
                    serde_json::from_str(&content).context(JsonSnafu)?;
                installed.push(strategy);
            }
        }

        Ok(installed)
    }
}

/// Parse a release tag like "btc-momentum-v0.1.0" into (name, version).
///
/// Expects the pattern `{name}-v{semver}` where version starts with a digit
/// after the `v`.
fn parse_tag(tag: &str) -> Option<(String, String)> {
    let idx = tag.rfind("-v")?;
    let version = &tag[idx + 1..]; // includes the "v"
    // Ensure there's a digit after "v" (not just any word starting with v)
    if version.len() < 2 || !version.as_bytes()[1].is_ascii_digit() {
        return None;
    }
    let name = &tag[..idx];
    Some((name.to_string(), version.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tag_extracts_name_and_version() {
        let (name, version) = parse_tag("btc-momentum-v0.1.0").unwrap();
        assert_eq!(name, "btc-momentum");
        assert_eq!(version, "v0.1.0");
    }

    #[test]
    fn parse_tag_handles_complex_names() {
        let (name, version) = parse_tag("hmm-regime-v0.1.0").unwrap();
        assert_eq!(name, "hmm-regime");
        assert_eq!(version, "v0.1.0");
    }

    #[test]
    fn parse_tag_returns_none_for_invalid() {
        assert!(parse_tag("no-version-here").is_none());
    }
}
