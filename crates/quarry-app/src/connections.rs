// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! Saved connection profiles: JSON in ~/.config/quarry, passwords in the OS keychain.

use crate::state::{ConnectionProfile, ConnectionTargetKind};
use std::path::PathBuf;

const KEYCHAIN_SERVICE: &str = "quarry";

/// `~/.config/quarry` (or `$XDG_CONFIG_HOME/quarry`)
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("quarry"))
}

fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("connections.json"))
}

pub fn load() -> Vec<ConnectionProfile> {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes every persistable profile. Passwords move to the keychain and never touch the file (C6).
pub fn save(profiles: &[ConnectionProfile]) -> Result<(), String> {
    let mut stored = Vec::new();
    for p in profiles.iter().filter(|p| p.kind != ConnectionTargetKind::SqliteMemory) {
        let (path_or_url, password) = split_password(&p.path_or_url);
        if let Some(pw) = password {
            keyring::Entry::new(KEYCHAIN_SERVICE, &p.id)
                .and_then(|e| e.set_password(&pw))
                .map_err(|e| format!("keychain: {e}"))?;
        }
        stored.push(ConnectionProfile { path_or_url, ..p.clone() });
    }
    let path = config_path().ok_or("no home directory")?;
    std::fs::create_dir_all(path.parent().expect("has parent")).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&stored).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Returns the connection string with its keychain password restored (if one is stored).
pub fn with_secret(profile: &ConnectionProfile) -> String {
    if profile.kind != ConnectionTargetKind::Postgres || split_password(&profile.path_or_url).1.is_some() {
        return profile.path_or_url.clone();
    }
    match keyring::Entry::new(KEYCHAIN_SERVICE, &profile.id).and_then(|e| e.get_password()) {
        Ok(pw) => insert_password(&profile.path_or_url, &pw),
        Err(_) => profile.path_or_url.clone(),
    }
}

/// Splits the password out of a Postgres URL (`postgres://user:pw@host/db`) or
/// key/value string (`host=x password=pw`).
pub fn split_password(conn: &str) -> (String, Option<String>) {
    if let Some((scheme, rest)) = conn.split_once("://") {
        let authority_end = rest.find('/').unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(authority_end);
        if let Some((userinfo, host)) = authority.rsplit_once('@') {
            if let Some((user, pw)) = userinfo.split_once(':') {
                return (format!("{scheme}://{user}@{host}{tail}"), Some(pw.to_string()));
            }
        }
        return (conn.to_string(), None);
    }
    // ponytail: whitespace-split; quoted values containing spaces aren't handled
    let mut password = None;
    let kept: Vec<&str> = conn
        .split_whitespace()
        .filter(|tok| match tok.strip_prefix("password=") {
            Some(pw) => {
                password = Some(pw.to_string());
                false
            }
            None => true,
        })
        .collect();
    (kept.join(" "), password)
}

fn insert_password(conn: &str, pw: &str) -> String {
    match conn.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((user, host)) => format!("{scheme}://{user}:{pw}@{host}"),
            None => conn.to_string(),
        },
        None => format!("{conn} password={pw}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_round_trips_out_of_and_into_connection_strings() {
        let url = "postgres://app:s3cr%40t@db.example.com:5432/prod?sslmode=require";
        let (bare, pw) = split_password(url);
        assert_eq!(bare, "postgres://app@db.example.com:5432/prod?sslmode=require");
        assert_eq!(pw.as_deref(), Some("s3cr%40t"));
        assert_eq!(insert_password(&bare, &pw.unwrap()), url);

        assert_eq!(split_password("postgres://app@host/db"), ("postgres://app@host/db".into(), None));

        let (kv, pw) = split_password("host=localhost password=hunter2 dbname=x");
        assert_eq!((kv.as_str(), pw.as_deref()), ("host=localhost dbname=x", Some("hunter2")));
    }
}
