// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! The driver registry. A driver turns a connection string into a session and describes the
//! connect form, so the app never names a database. Adding one: implement `Driver`, add it to
//! `drivers()` behind a cargo feature. See DRIVERS.md.

use crate::session::{DatabaseSession, DbError};
use async_trait::async_trait;
use std::sync::Arc;

/// One input in the connect form. Fields with the same `row` sit side by side.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub label: &'static str,
    pub placeholder: &'static str,
    /// Masked in the form and stored in the OS keychain, never in connections.json
    pub secret: bool,
    pub row: u8,
    /// Fixed width in px; `None` shares the row
    pub width: Option<f32>,
}

#[async_trait]
pub trait Driver: Send + Sync {
    /// Stable id stored in saved connections, e.g. "postgres"
    fn id(&self) -> &'static str;

    /// Shown in the connect form, e.g. "PostgreSQL"
    fn name(&self) -> &'static str;

    /// Prefilled when the user picks this driver in the form
    fn sample(&self) -> &'static str;

    /// Whether a pasted string is meant for this driver. Checked in registry order.
    fn recognizes(&self, conn: &str) -> bool;

    /// The structured form fields. A single field means the connection string is typed directly.
    fn fields(&self) -> &'static [Field];

    /// Connection string → one value per field
    fn split(&self, conn: &str) -> Vec<String>;

    /// Field values → connection string. `previous` keeps whatever the fields don't cover (options).
    fn join(&self, values: &[String], previous: &str) -> String;

    /// The connection string without its password, and the password
    fn take_secret(&self, conn: &str) -> (String, Option<String>) {
        (conn.to_string(), None)
    }

    /// Puts a keychain password back into a bare connection string
    fn put_secret(&self, conn: &str, _secret: &str) -> String {
        conn.to_string()
    }

    /// Nothing outlives the session (an in-memory database), so the connection isn't saved
    fn is_ephemeral(&self, _conn: &str) -> bool {
        false
    }

    /// Short "where" for the sidebar and default names: `host:port/db`, a file name
    fn display_address(&self, conn: &str) -> String;

    /// Host and port an SSH tunnel should forward to; `None` when the database isn't reached over TCP
    fn tunnel_target(&self, _conn: &str) -> Option<(String, u16)> {
        None
    }

    /// The connection string pointed at the tunnel's local end
    fn through_tunnel(&self, conn: &str, _local_port: u16) -> String {
        conn.to_string()
    }

    async fn connect(
        &self,
        conn: &str,
        read_only: bool,
    ) -> Result<Arc<dyn DatabaseSession>, DbError>;
}

/// Every driver compiled in. SQLite is last: it takes any path, so it's the fallback for pasted text.
static DRIVERS: &[&dyn Driver] = &[
    #[cfg(feature = "postgres")]
    &crate::postgres::DRIVER,
    #[cfg(feature = "mysql")]
    &crate::mysql::DRIVER,
    #[cfg(feature = "mssql")]
    &crate::mssql::DRIVER,
    #[cfg(feature = "oracle")]
    &crate::oracle::DRIVER,
    #[cfg(feature = "sqlite")]
    &crate::sqlite::DRIVER,
];

pub fn drivers() -> &'static [&'static dyn Driver] {
    DRIVERS
}

pub fn driver(id: &str) -> Option<&'static dyn Driver> {
    drivers().iter().copied().find(|d| d.id() == id)
}

/// The driver a pasted connection string is for
pub fn detect(conn: &str) -> Option<&'static dyn Driver> {
    drivers()
        .iter()
        .copied()
        .find(|d| d.recognizes(conn.trim()))
}

#[cfg(all(test, feature = "postgres", feature = "sqlite"))]
mod tests {
    use super::*;

    #[test]
    fn detects_the_driver_for_pasted_text() {
        assert_eq!(
            detect("postgres://u@h/db").map(|d| d.id()),
            Some("postgres")
        );
        assert_eq!(
            detect("host=db port=5432").map(|d| d.id()),
            Some("postgres")
        );
        assert_eq!(detect("~/data/app.sqlite").map(|d| d.id()), Some("sqlite"));
        assert_eq!(detect(":memory:").map(|d| d.id()), Some("sqlite"));
    }

    #[test]
    fn every_driver_round_trips_its_own_sample() {
        for d in drivers() {
            let fields = d.split(d.sample());
            assert_eq!(fields.len(), d.fields().len(), "{}", d.id());
            assert_eq!(d.join(&fields, d.sample()), d.sample(), "{}", d.id());
            assert!(d.recognizes(d.sample()), "{}", d.id());
        }
    }
}
