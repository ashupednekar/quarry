// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Self-update from the latest GitHub release's `latest.json` (written by the release workflow)
use cargo_packager_updater::{Config, Update};

const ENDPOINT: &str = "https://github.com/thekodeking/quarry/releases/latest/download/latest.json";
/// Minisign public key; the private half signs release bundles in CI
const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEUyMzIyMkEzNDQzRTczNDMKUldSRGN6NUVveUl5NHNyeFZSa2YrVGZaREl6dDBxdUVKU3ZlWXlSZkVFdSs3NDNoa3hOMzJESUoK";

/// Blocking. None for dev builds, .deb installs (no $APPIMAGE), or when already current
pub fn check() -> Option<Update> {
    if cfg!(debug_assertions)
        || (cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none())
    {
        return None;
    }
    let config = Config {
        endpoints: vec![ENDPOINT.parse().ok()?],
        pubkey: PUBKEY.into(),
        ..Default::default()
    };
    cargo_packager_updater::check_update(env!("CARGO_PKG_VERSION").parse().ok()?, config)
        .inspect_err(|e| tracing::warn!("update check failed: {e}"))
        .ok()
        .flatten()
}

/// Blocking. On Windows the installer takes over and this process exits; elsewhere the new
/// copy is started and the caller quits this one
pub fn install(update: &Update) -> anyhow::Result<()> {
    update.download_and_install()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg("-n")
        .arg(&update.extract_path)
        .spawn()?;
    #[cfg(target_os = "linux")]
    std::process::Command::new(&update.extract_path).spawn()?;
    Ok(())
}
