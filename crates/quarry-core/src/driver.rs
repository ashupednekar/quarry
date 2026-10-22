// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

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

/// Database driver abstraction responsible for connection instantiation and UI metadata.
///
/// Implementors declare driver identification, connection string URL syntax, recognition heuristics,
/// connection configuration form fields, password extraction/keychain management, SSH tunneling targets,
/// and asynchronous connection factory logic.
#[async_trait]
pub trait Driver: Send + Sync {
    /// Stable identifier stored in saved connections (e.g. `"postgres"`, `"sqlite"`, `"mysql"`).
    fn id(&self) -> &'static str;

    /// Human-readable name displayed in the connection manager UI (e.g. `"PostgreSQL"`).
    fn name(&self) -> &'static str;

    /// Prefilled sample connection string shown when the user selects this driver.
    fn sample(&self) -> &'static str;

    /// Evaluates whether a raw connection string belongs to this driver.
    ///
    /// # Arguments
    ///
    /// * `conn` - Raw connection string or file path to evaluate.
    ///
    /// # Returns
    ///
    /// `true` if this driver recognizes the URI scheme or connection syntax.
    fn recognizes(&self, conn: &str) -> bool;

    /// The structured connection form fields.
    ///
    /// A single field indicates the connection string is edited directly without decomposition.
    fn fields(&self) -> &'static [Field];

    /// Deconstructs a connection string into one value per form field.
    ///
    /// # Arguments
    ///
    /// * `conn` - Serialized connection string.
    ///
    /// # Returns
    ///
    /// Vector of field string values matching the ordering of [`fields`](Self::fields).
    fn split(&self, conn: &str) -> Vec<String>;

    /// Recombines form field values into a connection string.
    ///
    /// # Arguments
    ///
    /// * `values` - Slice of field values matching the driver's [`Field`] layout.
    /// * `previous` - Previous connection string preserving options not covered by form fields.
    ///
    /// # Returns
    ///
    /// Formatted connection string.
    fn join(&self, values: &[String], previous: &str) -> String;

    /// Extracts sensitive secrets (passwords) from the connection string for OS keychain storage.
    ///
    /// # Arguments
    ///
    /// * `conn` - Raw connection string containing potential credentials.
    ///
    /// # Returns
    ///
    /// A tuple of `(sanitized_connection_string, optional_extracted_password)`.
    fn take_secret(&self, conn: &str) -> (String, Option<String>) {
        (conn.to_string(), None)
    }

    /// Re-injects an OS keychain secret back into a sanitized connection string.
    ///
    /// # Arguments
    ///
    /// * `conn` - Sanitized connection string.
    /// * `secret` - Secret credentials retrieved from the system keychain.
    ///
    /// # Returns
    ///
    /// Complete connection string ready for driver connection.
    fn put_secret(&self, conn: &str, _secret: &str) -> String {
        conn.to_string()
    }

    /// Indicates whether database data is ephemeral and tied to process lifetime (e.g. SQLite `:memory:`).
    fn is_ephemeral(&self, _conn: &str) -> bool {
        false
    }

    /// Generates a concise display address for the sidebar and title (e.g. `host:port/db` or filename).
    ///
    /// # Arguments
    ///
    /// * `conn` - Serialized connection string.
    ///
    /// # Returns
    ///
    /// Short human-readable target string.
    fn display_address(&self, conn: &str) -> String;

    /// Extracts target host and port when connection is routable over an SSH tunnel.
    ///
    /// # Arguments
    ///
    /// * `conn` - Connection string.
    ///
    /// # Returns
    ///
    /// `Some((host, port))` for TCP endpoints, or `None` for local/embedded databases.
    fn tunnel_target(&self, _conn: &str) -> Option<(String, u16)> {
        None
    }

    /// Rewrites a connection string to route through a local SSH tunnel port.
    ///
    /// # Arguments
    ///
    /// * `conn` - Original connection string.
    /// * `local_port` - Local TCP port allocated for the active SSH tunnel.
    ///
    /// # Returns
    ///
    /// Rewritten connection string pointed at `127.0.0.1:<local_port>`.
    fn through_tunnel(&self, conn: &str, _local_port: u16) -> String {
        conn.to_string()
    }

    /// Establishes an asynchronous database session.
    ///
    /// # Arguments
    ///
    /// * `conn` - Connection string containing credentials and endpoint settings.
    /// * `read_only` - If `true`, the connection should enforce read-only semantics if supported.
    ///
    /// # Returns
    ///
    /// An atomically reference-counted [`DatabaseSession`] instance.
    ///
    /// # Errors
    ///
    /// * [`DbError::AuthenticationFailed`] if invalid credentials provided.
    /// * [`DbError::ConnectionLost`] if host is unreachable.
    /// * [`DbError::QueryError`] if initialization commands fail.
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
