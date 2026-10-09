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
    let mut profiles: Vec<ConnectionProfile> = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if fix_default_names(&mut profiles) {
        let _ = save(&profiles);
    }
    profiles
}

/// The add-connection form used to pre-fill "Local SQLite", so non-SQLite connections were saved under that name.
/// Renames them to their address. Returns whether anything changed.
fn fix_default_names(profiles: &mut [ConnectionProfile]) -> bool {
    let mut changed = false;
    for p in profiles.iter_mut().filter(|p| p.name == "Local SQLite" && p.kind == ConnectionTargetKind::Postgres) {
        p.name = display_address(p);
        changed = true;
    }
    changed
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

/// `host:port/db` for Postgres (no user or password), the file name for SQLite
pub fn display_address(p: &ConnectionProfile) -> String {
    match p.kind {
        ConnectionTargetKind::SqliteMemory => ":memory:".into(),
        ConnectionTargetKind::SqliteFile => std::path::Path::new(&p.path_or_url).file_name().map_or_else(|| p.path_or_url.clone(), |f| f.to_string_lossy().into_owned()),
        ConnectionTargetKind::Postgres => {
            let (bare, _) = split_password(&p.path_or_url);
            let rest = bare.split_once("://").map_or(bare.as_str(), |(_, r)| r);
            let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
            rest.split('?').next().unwrap_or(rest).to_string()
        }
    }
}

pub fn forget_secret(id: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, id) {
        let _ = entry.delete_credential(); // nothing stored is fine
    }
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
mod display_tests {
    use super::display_address;
    use crate::state::{ConnectionProfile, ConnectionTargetKind, Environment};

    #[test]
    fn shows_where_a_connection_points() {
        let p = |kind, url: &str| ConnectionProfile { id: "x".into(), name: "n".into(), kind, path_or_url: url.into(), env: Environment::Local, read_only: false, ssh: None, statement_timeout_secs: None };
        assert_eq!(display_address(&p(ConnectionTargetKind::Postgres, "postgres://me:secret@localhost:5432/app?sslmode=disable")), "localhost:5432/app");
        assert_eq!(display_address(&p(ConnectionTargetKind::SqliteFile, "/tmp/data/app.db")), "app.db");
    }

    #[test]
    fn renames_connections_saved_under_the_old_default() {
        let p = |name: &str, kind, url: &str| ConnectionProfile { id: "x".into(), name: name.into(), kind, path_or_url: url.into(), env: Environment::Local, read_only: false, ssh: None, statement_timeout_secs: None };
        let mut list = vec![
            p("Local SQLite", ConnectionTargetKind::Postgres, "postgres://postgres@localhost:5432/postgres"),
            p("Local SQLite", ConnectionTargetKind::SqliteFile, "/tmp/a.db"),
        ];
        assert!(super::fix_default_names(&mut list));
        assert_eq!((list[0].name.as_str(), list[1].name.as_str()), ("localhost:5432/postgres", "Local SQLite"));
        assert!(!super::fix_default_names(&mut list));
    }
}

/// Host, port, user, password, database: the fields the connection modal shows for a Postgres URL
pub type PgFields = [String; 5];

/// Splits `postgres://user:pw@host:port/db?query` into its fields (decoded) and the query string, kept as-is
pub fn parse_pg_url(url: &str) -> Option<(PgFields, String)> {
    let rest = url.trim().strip_prefix("postgres://").or_else(|| url.trim().strip_prefix("postgresql://"))?;
    let (rest, query) = rest.split_once('?').map_or((rest, ""), |(r, q)| (r, q));
    let (authority, db) = rest.split_once('/').unwrap_or((rest, ""));
    let (userinfo, hostport) = authority.rsplit_once('@').unwrap_or(("", authority));
    let (user, pw) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    // ponytail: no IPv6 brackets; `[::1]:5432` splits wrong, handle it if anyone asks
    let (host, port) = hostport.rsplit_once(':').unwrap_or((hostport, ""));
    Some(([host, port, user, pw, db].map(pct_decode), query.to_string()))
}

/// The inverse of `parse_pg_url`: blank parts are left out so the server defaults apply
pub fn build_pg_url(f: &PgFields, query: &str) -> String {
    let [host, port, user, pw, db] = f.each_ref().map(|s| s.trim());
    let mut url = String::from("postgres://");
    if !user.is_empty() {
        url += &pct_encode(user);
        if !pw.is_empty() {
            url += &format!(":{}", pct_encode(pw));
        }
        url.push('@');
    }
    url += host;
    if !port.is_empty() {
        url += &format!(":{port}");
    }
    if !db.is_empty() {
        url += &format!("/{}", pct_encode(db));
    }
    if !query.is_empty() {
        url += &format!("?{query}");
    }
    url
}

fn pct_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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

    #[test]
    fn pg_url_round_trips_through_fields() {
        let (f, q) = parse_pg_url("postgres://app:s3cr%40t@db.example.com:5432/prod?sslmode=require").unwrap();
        assert_eq!(f, ["db.example.com", "5432", "app", "s3cr@t", "prod"].map(String::from));
        assert_eq!(build_pg_url(&f, &q), "postgres://app:s3cr%40t@db.example.com:5432/prod?sslmode=require");
        assert_eq!(build_pg_url(&["localhost", "", "", "", ""].map(String::from), ""), "postgres://localhost");
        assert!(parse_pg_url("/tmp/x.db").is_none());
    }
}
