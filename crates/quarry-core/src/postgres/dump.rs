// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Whole-database dump through pg_dump (X3)

use super::url::{pct_decode, split_password};
use super::PostgresSession;
use crate::dump::Dumper;
use async_trait::async_trait;
use std::path::Path;

#[async_trait]
impl Dumper for PostgresSession {
    /// The password goes through PGPASSWORD so it never shows in the process list
    async fn dump(&self, out: &Path) -> Result<String, String> {
        let (bare, password) = split_password(&self.url);
        let mut cmd = crate::command("pg_dump");
        cmd.args(["--no-owner", "--dbname", &bare, "--file"])
            .arg(out);
        if let Some(pw) = password {
            // A URL carries the password percent-encoded; the environment variable wants it plain
            cmd.env(
                "PGPASSWORD",
                if bare.contains("://") {
                    pct_decode(&pw)
                } else {
                    pw
                },
            );
        }
        let output = tokio::task::spawn_blocking(move || cmd.output())
            .await
            .map_err(|e| e.to_string())?;
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
