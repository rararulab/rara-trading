//! Shared HTTP client.

use std::{sync::OnceLock, time::Duration};

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static DOWNLOAD_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Shared HTTP client with sensible defaults.
pub fn client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client initialization should not fail")
    })
}

/// HTTP client for large file downloads.
///
/// Uses a connect-only timeout (no total-request timeout) and disables
/// automatic decompression to avoid conflicts with binary payloads.
pub fn download_client() -> &'static reqwest::Client {
    DOWNLOAD_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .connect_timeout(Duration::from_secs(30))
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .build()
            .expect("download client initialization should not fail")
    })
}
