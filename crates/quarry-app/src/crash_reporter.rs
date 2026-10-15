// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Crash reporting and panic handling.
//!
//! Captures uncaught Rust panics, formats system metadata, stack backtraces,
//! and sanitized recent breadcrumbs into a local JSON report.
//! Provides prefilled GitHub issue URL generation and clipboard report formatting.

use serde::{Deserialize, Serialize};
use std::backtrace::Backtrace;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CrashReport {
    pub timestamp: String,
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub panic_message: String,
    pub location: Option<String>,
    pub backtrace: String,
    pub breadcrumbs: Vec<String>,
}

static BREADCRUMBS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Adds a breadcrumb (log or action) to the crash reporter's ring buffer.
pub fn add_breadcrumb(crumb: impl Into<String>) {
    if let Ok(mut lock) = BREADCRUMBS.lock() {
        if lock.len() >= 40 {
            lock.remove(0);
        }
        lock.push(crumb.into());
    }
}

/// Directory where crash reports are stored: `~/.config/quarry/crashes/`
pub fn crash_dir() -> Option<PathBuf> {
    Some(crate::connections::config_dir()?.join("crashes"))
}

/// Path to the last recorded crash report
pub fn last_crash_path() -> Option<PathBuf> {
    Some(crash_dir()?.join("last_crash.json"))
}

/// Sets up the global panic hook to capture crash reports
pub fn init() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let timestamp = chrono::Utc::now().to_rfc3339();
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<dyn Any>".to_string()
        };

        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));

        let bt = Backtrace::force_capture().to_string();
        let crumbs = BREADCRUMBS.lock().map(|b| b.clone()).unwrap_or_default();

        let report = CrashReport {
            timestamp,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            panic_message: payload,
            location,
            backtrace: bt,
            breadcrumbs: crumbs,
        };

        if let Some(dir) = crash_dir() {
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(json) = serde_json::to_string_pretty(&report) {
                let _ = std::fs::write(dir.join("last_crash.json"), &json);
                let ts_file = format!("crash_{}.json", chrono::Utc::now().timestamp());
                let _ = std::fs::write(dir.join(ts_file), &json);
            }
        }

        default_hook(info);
    }));
}

/// Checks if a crash report from the previous session exists
pub fn check_last_crash() -> Option<CrashReport> {
    let path = last_crash_path()?;
    if path.exists() {
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    } else {
        None
    }
}

/// Loads the latest crash report, either from `last_crash.json` or by searching for the newest `crash_*.json` in the crash directory.
pub fn load_latest_crash() -> Option<CrashReport> {
    if let Some(report) = check_last_crash() {
        return Some(report);
    }
    let dir = crash_dir()?;
    let entries = std::fs::read_dir(dir).ok()?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.starts_with("crash_") && s.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    if let Some(latest) = files.last() {
        let content = std::fs::read_to_string(latest).ok()?;
        serde_json::from_str(&content).ok()
    } else {
        None
    }
}

/// Removes `last_crash.json` so the user is not prompted again
pub fn acknowledge_last_crash() {
    if let Some(path) = last_crash_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Formats the crash report as GitHub-flavored Markdown for copying
pub fn format_markdown_report(report: &CrashReport) -> String {
    let loc = report.location.as_deref().unwrap_or("unknown");
    let mut out = format!(
        "### Crash Report\n\n\
        - **Quarry Version:** {}\n\
        - **OS:** {}\n\
        - **Arch:** {}\n\
        - **Timestamp:** {}\n\
        - **Location:** `{}`\n\
        - **Panic:** `{}`\n\n",
        report.app_version, report.os, report.arch, report.timestamp, loc, report.panic_message
    );

    if !report.breadcrumbs.is_empty() {
        out.push_str("#### Recent Breadcrumbs\n```\n");
        for b in &report.breadcrumbs {
            out.push_str(b);
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    out.push_str(
        "#### Backtrace\n<details><summary>Click to view stack backtrace</summary>\n\n```\n",
    );
    out.push_str(&report.backtrace);
    out.push_str("\n```\n</details>\n");
    out
}

/// Generates a prefilled GitHub Issue creation URL
pub fn format_github_issue_url(report: &CrashReport) -> String {
    let loc = report.location.as_deref().unwrap_or("unknown");
    let title = format!("[Crash]: {} at {}", report.panic_message, loc);

    let mut truncated_bt = report
        .backtrace
        .lines()
        .take(25)
        .collect::<Vec<_>>()
        .join("\n");
    if report.backtrace.lines().count() > 25 {
        truncated_bt.push_str("\n... [truncated, full backtrace available in clipboard]");
    }

    let body = format!(
        "**Describe the bug:**\nQuarry crashed unexpectedly.\n\n\
        **System Information:**\n\
        - Quarry Version: {}\n\
        - OS: {}\n\
        - Arch: {}\n\
        - Location: `{}`\n\
        - Error: `{}`\n\n\
        **Backtrace:**\n```\n{}\n```\n",
        report.app_version, report.os, report.arch, loc, report.panic_message, truncated_bt
    );

    format!(
        "https://github.com/thekodeking/quarry/issues/new?title={}&body={}",
        urlencode(&title),
        urlencode(&body),
    )
}

fn urlencode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => {
                encoded.push_str("%20");
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencode() {
        assert_eq!(urlencode("hello world"), "hello%20world");
        assert_eq!(urlencode("[Crash]: test"), "%5BCrash%5D%3A%20test");
    }

    #[test]
    fn test_markdown_and_github_url() {
        let report = CrashReport {
            timestamp: "2026-10-14T19:00:00Z".into(),
            app_version: "0.1.0".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            panic_message: "assertion failed".into(),
            location: Some("src/main.rs:10:5".into()),
            backtrace: "stack backtrace:\n   0: test".into(),
            breadcrumbs: vec!["Opened tab".into()],
        };

        let md = format_markdown_report(&report);
        assert!(md.contains("assertion failed"));
        assert!(md.contains("Opened tab"));

        let url = format_github_issue_url(&report);
        assert!(url.starts_with("https://github.com/thekodeking/quarry/issues/new?title="));
        assert!(url.contains("assertion%20failed"));
    }

    #[test]
    fn test_breadcrumbs_ring_buffer() {
        for i in 0..50 {
            add_breadcrumb(format!("step {}", i));
        }
        let crumbs = BREADCRUMBS.lock().unwrap().clone();
        assert_eq!(crumbs.len(), 40);
        assert_eq!(crumbs.last().unwrap(), "step 49");
    }

    #[test]
    fn test_crash_report_serde() {
        let report = CrashReport {
            timestamp: "2026-10-14T19:00:00Z".into(),
            app_version: "0.1.0".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            panic_message: "panic message".into(),
            location: Some("foo.rs:1:1".into()),
            backtrace: "backtrace text".into(),
            breadcrumbs: vec!["b1".into(), "b2".into()],
        };

        let json = serde_json::to_string(&report).expect("serialize");
        let deserialized: CrashReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.panic_message, "panic message");
        assert_eq!(deserialized.breadcrumbs.len(), 2);
    }
}
