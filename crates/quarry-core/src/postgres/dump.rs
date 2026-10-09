// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Whole-database dump through pg_dump (X3)

use super::PostgresSession;
use crate::dump::Dumper;
use async_trait::async_trait;
use std::path::Path;

#[async_trait]
impl Dumper for PostgresSession {
    /// The password goes through PGPASSWORD so it never shows in the process list
    async fn dump(&self, out: &Path) -> Result<String, String> {
        let (bare, password) = split_password(&self.url);
        let mut cmd = std::process::Command::new("pg_dump");
        cmd.args(["--no-owner", "--dbname", &bare, "--file"]).arg(out);
        if let Some(pw) = password {
            cmd.env("PGPASSWORD", pw);
        }
        let output = tokio::task::spawn_blocking(move || cmd.output()).await.map_err(|e| e.to_string())?;
        match output {
            Ok(o) if o.status.success() => Ok("the database".into()),
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                Err(if err.contains("server version mismatch") {
                    format!("{err}. Install a pg_dump at least as new as the server (e.g. `brew install postgresql@16`).")
                } else {
                    err
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err("pg_dump isn't installed. Install the PostgreSQL client tools (e.g. `brew install libpq`) and try again.".into())
            }
            Err(e) => Err(e.to_string()),
        }
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

