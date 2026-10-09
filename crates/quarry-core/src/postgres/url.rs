// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Postgres connection strings: URLs (`postgres://user:pw@host:port/db?opts`) and
//! key/value strings (`host=x port=5432 dbname=y`)

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


pub(super) fn insert_password(conn: &str, pw: &str) -> String {
    match conn.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((user, host)) => format!("{scheme}://{user}:{pw}@{host}"),
            None => conn.to_string(),
        },
        None => format!("{conn} password={pw}"),
    }
}

/// Host, port, user, password, database: the fields the connection modal shows for a Postgres URL
pub(super) type PgFields = [String; 5];

/// Splits `postgres://user:pw@host:port/db?query` into its fields (decoded) and the query string, kept as-is
pub(super) fn parse_pg_url(url: &str) -> Option<(PgFields, String)> {
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
pub(super) fn build_pg_url(f: &PgFields, query: &str) -> String {
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

/// Key/value connection string → the same fields as a URL
pub(super) fn parse_kv(conn: &str) -> PgFields {
    // ponytail: whitespace-split; quoted values containing spaces aren't handled
    let get = |k: &str| conn.split_whitespace().find_map(|t| t.strip_prefix(&format!("{k}=")).map(str::to_string)).unwrap_or_default();
    [get("host"), get("port"), get("user"), get("password"), get("dbname")]
}

fn pct_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

pub(super) fn pct_decode(s: &str) -> String {
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

/// Host and port a Postgres connection string points at (defaults: localhost:5432)
pub(super) fn target(conn: &str) -> (String, u16) {
    if let Some((_, rest)) = conn.split_once("://") {
        let authority = rest.split(['/', '?']).next().unwrap_or("");
        let hostport = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
        return match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().unwrap_or(5432)),
            None => (if hostport.is_empty() { "localhost".into() } else { hostport.to_string() }, 5432),
        };
    }
    let get = |k: &str| conn.split_whitespace().find_map(|t| t.strip_prefix(&format!("{k}=")).map(str::to_string));
    (get("host").unwrap_or_else(|| "localhost".into()), get("port").and_then(|p| p.parse().ok()).unwrap_or(5432))
}

/// The same connection string pointed at 127.0.0.1:`port` (the tunnel's local end)
pub(super) fn retarget(conn: &str, port: u16) -> String {
    if let Some((scheme, rest)) = conn.split_once("://") {
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        let userinfo = authority.rsplit_once('@').map(|(u, _)| format!("{u}@")).unwrap_or_default();
        return format!("{scheme}://{userinfo}127.0.0.1:{port}{tail}");
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
        let (f, q) = parse_pg_url("postgres://app:s3cr%40t@db.example.com:5432/prod?sslmode=require").unwrap();
        assert_eq!(f, ["db.example.com", "5432", "app", "s3cr@t", "prod"].map(String::from));
        assert_eq!(build_pg_url(&f, &q), "postgres://app:s3cr%40t@db.example.com:5432/prod?sslmode=require");
        assert_eq!(build_pg_url(&["localhost", "", "", "", ""].map(String::from), ""), "postgres://localhost");
        assert!(parse_pg_url("/tmp/x.db").is_none());
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
