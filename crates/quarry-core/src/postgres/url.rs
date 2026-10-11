// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Postgres connection strings: URLs (`postgres://…`, see `conn_url`) and key/value strings
//! (`host=x port=5432 dbname=y`)

pub(super) use crate::conn_url::pct_decode;
use crate::conn_url::{self, UrlFields};

pub(super) const SCHEMES: [&str; 2] = ["postgres", "postgresql"];

/// Splits the password out of a Postgres URL or key/value string (`host=x password=pw`)
pub fn split_password(conn: &str) -> (String, Option<String>) {
    if let Some(split) = conn_url::split_password(conn) {
        return split;
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

pub(super) fn insert_password(conn: &str, pw: &str) -> String {
    if conn.contains("://") { conn_url::insert_password(conn, pw) } else { format!("{conn} password={pw}") }
}

/// Key/value connection string → the same fields as a URL
pub(super) fn parse_kv(conn: &str) -> UrlFields {
    // ponytail: whitespace-split; quoted values containing spaces aren't handled
    let get = |k: &str| conn.split_whitespace().find_map(|t| t.strip_prefix(&format!("{k}=")).map(str::to_string)).unwrap_or_default();
    [get("host"), get("port"), get("user"), get("password"), get("dbname")]
}

/// Host and port a connection string points at (defaults: localhost:5432)
pub(super) fn target(conn: &str) -> (String, u16) {
    conn_url::target(conn, 5432).unwrap_or_else(|| {
        let get = |k: &str| conn.split_whitespace().find_map(|t| t.strip_prefix(&format!("{k}=")).map(str::to_string));
        (get("host").unwrap_or_else(|| "localhost".into()), get("port").and_then(|p| p.parse().ok()).unwrap_or(5432))
    })
}

/// The same connection string pointed at 127.0.0.1:`port` (the tunnel's local end)
pub(super) fn retarget(conn: &str, port: u16) -> String {
    if conn.contains("://") {
        return conn_url::retarget(conn, port);
    }
    let kept: Vec<&str> = conn.split_whitespace().filter(|t| !t.starts_with("host=") && !t.starts_with("port=")).collect();
    format!("{} host=127.0.0.1 port={port}", kept.join(" ")).trim().to_string()
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
        let (f, q) = conn_url::parse("postgres://app:s3cr%40t@db.example.com:5432/prod?sslmode=require", &SCHEMES).unwrap();
        assert_eq!(f, ["db.example.com", "5432", "app", "s3cr@t", "prod"].map(String::from));
        assert_eq!(conn_url::build("postgres", &f, &q), "postgres://app:s3cr%40t@db.example.com:5432/prod?sslmode=require");
        assert_eq!(conn_url::build("postgres", &["localhost", "", "", "", ""].map(String::from), ""), "postgres://localhost");
        assert!(conn_url::parse("/tmp/x.db", &SCHEMES).is_none());
    }

    #[test]
    fn finds_and_rewrites_the_database_address() {
        let url = "postgres://app:pw@db.internal:6543/prod?sslmode=disable";
        assert_eq!(target(url), ("db.internal".into(), 6543));
        assert_eq!(retarget(url, 40001), "postgres://app:pw@127.0.0.1:40001/prod?sslmode=disable");
        assert_eq!(target("postgres://app@db"), ("db".into(), 5432));
        assert_eq!(target("host=10.0.0.5 port=5433 dbname=x"), ("10.0.0.5".into(), 5433));
        assert_eq!(retarget("host=10.0.0.5 dbname=x", 40002), "dbname=x host=127.0.0.1 port=40002");
    }

    #[test]
    fn key_value_strings_fill_the_same_fields() {
        assert_eq!(parse_kv("host=db port=6432 user=app dbname=shop"), ["db", "6432", "app", "", "shop"].map(String::from));
    }
}
