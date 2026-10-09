// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Postgres log lines split into time / pid / level / message, following the server's `log_line_prefix`.

/// (time, pid, level, message). A line that doesn't fit the prefix (a continuation, a custom format we
/// can't follow) comes back whole as the message.
pub fn parse_line(prefix: &str, line: &str) -> (String, String, String, String) {
    parse_prefix(prefix, line)
        .and_then(|(time, pid, rest)| {
            let (level, msg) = rest.split_once(':')?;
            let level = level.trim();
            level.chars().all(|c| c.is_ascii_uppercase()).then(|| (time, pid, level.to_string(), msg.trim_start().to_string()))
        })
        .unwrap_or_else(|| (String::new(), String::new(), String::new(), line.to_string()))
}

/// Walks the prefix: literal text must match; each `%x` escape takes the text up to the next literal
/// (timestamps take their three words). Returns (time, pid, rest of line).
fn parse_prefix<'a>(prefix: &str, line: &'a str) -> Option<(String, String, &'a str)> {
    let (mut time, mut pid) = (String::new(), String::new());
    let mut rest = line;
    let mut chars = prefix.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            rest = rest.strip_prefix(c)?;
            continue;
        }
        let esc = chars.next()?;
        if esc == '%' {
            rest = rest.strip_prefix('%')?;
            continue;
        }
        if esc == 'q' {
            continue; // "stop here in non-session processes": nothing printed
        }
        // Up to the next literal in the prefix (or a space when the prefix ends here).
        // Timestamps (`2026-10-09 01:43:31.123 IST`) hold two spaces of their own, so the search starts after them.
        let stop = chars.peek().copied().filter(|c| *c != '%').unwrap_or(' ');
        let skip = if matches!(esc, 'm' | 't') { rest.match_indices(' ').nth(1).map_or(0, |(i, _)| i + 1) } else { 0 };
        let value = &rest[..rest[skip..].find(stop).map_or(rest.len(), |i| skip + i)];
        match esc {
            'm' | 't' | 'n' => time = value.to_string(),
            'p' => pid = value.to_string(),
            _ => {}
        }
        rest = &rest[value.len()..];
    }
    Some((time, pid, rest.trim_start()))
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn default_prefix() {
        let (t, p, l, m) = parse_line("%m [%p] ", "2026-10-09 01:43:31.123 IST [4242] ERROR:  relation \"x\" does not exist");
        assert_eq!((t.as_str(), p.as_str(), l.as_str(), m.as_str()), ("2026-10-09 01:43:31.123 IST", "4242", "ERROR", "relation \"x\" does not exist"));
    }

    #[test]
    fn custom_prefix_and_continuations() {
        let line = "2026-10-09 01:43:31 UTC:10.0.0.1(5432):app@shop:[77]:LOG:  checkpoint starting";
        let (t, p, l, m) = parse_line("%t:%r:%u@%d:[%p]:", line);
        assert_eq!((t.as_str(), p.as_str(), l.as_str(), m.as_str()), ("2026-10-09 01:43:31 UTC", "77", "LOG", "checkpoint starting"));
        // A STATEMENT continuation line has no prefix: kept whole
        assert_eq!(parse_line("%m [%p] ", "\tSELECT 1").3, "\tSELECT 1");
    }
}
