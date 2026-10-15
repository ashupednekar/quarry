// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! `scheme://user:password@host:port/database?options` connection URLs, shared by the drivers
//! that use them (Postgres, MySQL). Field order everywhere: host, port, user, password, database.

pub type UrlFields = [String; 5];

/// The URL's fields (decoded) and its query string (kept as-is), if it starts with one of `schemes`
pub fn parse(url: &str, schemes: &[&str]) -> Option<(UrlFields, String)> {
    let url = url.trim();
    let rest = schemes
        .iter()
        .find_map(|s| url.strip_prefix(s).and_then(|r| r.strip_prefix("://")))?;
    let (rest, query) = rest.split_once('?').map_or((rest, ""), |(r, q)| (r, q));
    let (authority, db) = rest.split_once('/').unwrap_or((rest, ""));
    let (userinfo, hostport) = authority.rsplit_once('@').unwrap_or(("", authority));
    let (user, pw) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    // ponytail: no IPv6 brackets; `[::1]:5432` splits wrong, handle it if anyone asks
    let (host, port) = hostport.rsplit_once(':').unwrap_or((hostport, ""));
    Some((
        [host, port, user, pw, db].map(pct_decode),
        query.to_string(),
    ))
}

/// The inverse of `parse`: blank parts are left out so the server defaults apply
pub fn build(scheme: &str, f: &UrlFields, query: &str) -> String {
    let [host, port, user, pw, db] = f.each_ref().map(|s| s.trim());
    let mut url = format!("{scheme}://");
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

/// The URL without its password, and the password (still percent-encoded); `None` if not a URL
pub fn split_password(url: &str) -> Option<(String, Option<String>)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if let Some((userinfo, host)) = authority.rsplit_once('@') {
        if let Some((user, pw)) = userinfo.split_once(':') {
            return Some((
                format!("{scheme}://{user}@{host}{tail}"),
                Some(pw.to_string()),
            ));
        }
    }
    Some((url.to_string(), None))
}

/// Puts a password back into a URL that has a user and none
pub fn insert_password(url: &str, pw: &str) -> String {
    match url.split_once("://").and_then(|(scheme, rest)| {
        rest.split_once('@')
            .map(|(user, host)| (scheme, user, host))
    }) {
        Some((scheme, user, host)) => format!("{scheme}://{user}:{pw}@{host}"),
        None => url.to_string(),
    }
}

/// Host and port a URL points at, with the database's default port
pub fn target(url: &str, default_port: u16) -> Option<(String, u16)> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    let hostport = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    Some(match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(default_port)),
        None => (
            if hostport.is_empty() {
                "localhost".into()
            } else {
                hostport.to_string()
            },
            default_port,
        ),
    })
}

/// The same URL pointed at 127.0.0.1:`port` (an SSH tunnel's local end)
pub fn retarget(url: &str, port: u16) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let userinfo = authority
        .rsplit_once('@')
        .map(|(u, _)| format!("{u}@"))
        .unwrap_or_default();
    format!("{scheme}://{userinfo}127.0.0.1:{port}{tail}")
}

pub fn pct_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (
            b[i],
            s.get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok()),
        ) {
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
    fn fields_round_trip_through_any_scheme() {
        let url = "mysql://app:s3cr%40t@db.example.com:3306/shop?ssl-mode=required";
        let (f, q) = parse(url, &["mysql", "mariadb"]).unwrap();
        assert_eq!(
            f,
            ["db.example.com", "3306", "app", "s3cr@t", "shop"].map(String::from)
        );
        assert_eq!(build("mysql", &f, &q), url);
        assert!(parse(url, &["postgres"]).is_none());
        assert_eq!(target("mysql://h/db", 3306), Some(("h".into(), 3306)));
    }
}
