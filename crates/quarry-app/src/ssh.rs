// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! SSH tunnels (C4, UC2) via the system `ssh` client: agent or key file, no prompts.

use serde::{Deserialize, Serialize};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Private key file; `None` uses ssh-agent / ~/.ssh config
    pub key_path: Option<String>,
}

/// A running `ssh -L`; the process is stopped when this is dropped
pub struct Tunnel {
    child: Child,
    pub local_port: u16,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Tunnel {
    /// Forwards a free local port to `remote_host:remote_port` as seen from the SSH server.
    /// Retries up to 3 times to prevent transient port allocation race conditions (TOCTOU).
    pub fn open(cfg: &SshConfig, remote_host: &str, remote_port: u16) -> Result<Tunnel, String> {
        let mut last_err = String::new();
        for _ in 0..3 {
            match Self::try_open(cfg, remote_host, remote_port) {
                Ok(tunnel) => return Ok(tunnel),
                Err(e) => {
                    last_err = e;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        Err(last_err)
    }

    fn try_open(cfg: &SshConfig, remote_host: &str, remote_port: u16) -> Result<Tunnel, String> {
        let local_port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| e.to_string())?
            .port();
        let mut cmd = quarry_core::command("ssh");
        cmd.args([
            "-N",
            "-L",
            &format!("127.0.0.1:{local_port}:{remote_host}:{remote_port}"),
        ])
        .args(["-p", &cfg.port.to_string()])
        // Never prompt (passphrase keys must be in ssh-agent); fail fast if the forward can't be set up
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=30",
        ])
        .args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ConnectTimeout=10",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
        if let Some(key) = cfg.key_path.as_deref().filter(|k| !k.trim().is_empty()) {
            cmd.args(["-i", &expand_home(key), "-o", "IdentitiesOnly=yes"]);
        }
        cmd.arg(format!("{}@{}", cfg.user, cfg.host));
        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "ssh isn't installed or isn't on PATH".to_string(),
            _ => e.to_string(),
        })?;

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(Some(_)) = child.try_wait() {
                let mut err = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
                }
                return Err(format!("SSH tunnel failed: {}", err.trim()));
            }
            if TcpStream::connect(("127.0.0.1", local_port)).is_ok() {
                watch(std::process::id(), child.id());
                return Ok(Tunnel { child, local_port });
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                return Err(format!("SSH tunnel to {} timed out", cfg.host));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Drop only runs on a clean exit. This watchdog stops ssh if Quarry is killed or crashes,
/// and exits on its own once the tunnel is closed normally.
#[cfg(unix)]
fn watch(app_pid: u32, ssh_pid: u32) {
    let script = format!("while kill -0 {app_pid} 2>/dev/null && kill -0 {ssh_pid} 2>/dev/null; do sleep 2; done; kill {ssh_pid} 2>/dev/null");
    let _ = quarry_core::command("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(windows)]
fn watch(app_pid: u32, ssh_pid: u32) {
    let script = format!("while ((Get-Process -Id {app_pid} -EA 0) -and (Get-Process -Id {ssh_pid} -EA 0)) {{ Start-Sleep 2 }}; Stop-Process -Id {ssh_pid} -EA 0");
    let _ = quarry_core::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(not(any(unix, windows)))]
fn watch(_app_pid: u32, _ssh_pid: u32) {}

pub fn current_os_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

pub fn expand_home(path: &str) -> String {
    let stripped = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"));
    match stripped {
        Some(rest) => {
            if let Some(mut home) = std::env::home_dir() {
                home.push(rest.replace('\\', "/"));
                home.to_string_lossy().to_string()
            } else {
                path.to_string()
            }
        }
        None => path.to_string(),
    }
}

/// A configured host parsed from SSH config files or SSH agent
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

/// Discovers configured servers from `~/.ssh/config` (including `Include` directives)
pub fn discover_ssh_hosts() -> Vec<DiscoveredSshHost> {
    let mut seen = std::collections::HashSet::new();
    let mut hosts = Vec::new();
    let default_config = expand_home("~/.ssh/config");
    let path = std::path::Path::new(&default_config);
    if path.exists() {
        parse_ssh_config_file(path, &mut seen, &mut hosts);
    }
    hosts
}

fn parse_ssh_config_file(
    path: &std::path::Path,
    seen: &mut std::collections::HashSet<std::path::PathBuf>,
    hosts: &mut Vec<DiscoveredSshHost>,
) {
    let canonical = match path.canonicalize() {
        Ok(c) => c,
        Err(_) => path.to_path_buf(),
    };
    if seen.contains(&canonical) {
        return;
    }
    seen.insert(canonical);

    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };

    let mut current_indices: Vec<usize> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let Some(key) = tokens.next() else { continue };
        let key_lower = key.to_ascii_lowercase();

        if key_lower == "host" {
            current_indices.clear();
            for pattern in tokens {
                if !pattern.contains('*') && !pattern.contains('?') && !pattern.starts_with('!') {
                    let unquoted = pattern.trim_matches('"').trim_matches('\'');
                    if !unquoted.is_empty() {
                        let idx = hosts.len();
                        hosts.push(DiscoveredSshHost {
                            alias: unquoted.to_string(),
                            hostname: None,
                            user: None,
                            port: None,
                            identity_file: None,
                        });
                        current_indices.push(idx);
                    }
                }
            }
        } else if key_lower == "include" {
            for inc in tokens {
                let inc_expanded = expand_home(inc.trim_matches('"').trim_matches('\''));
                let inc_path = std::path::Path::new(&inc_expanded);
                let pattern = if inc_path.is_absolute() {
                    inc_expanded
                } else if let Some(parent) = path.parent() {
                    parent.join(inc_path).to_string_lossy().to_string()
                } else {
                    inc_expanded
                };
                if pattern.contains('*') || pattern.contains('?') {
                    if let Some((dir, file_pattern)) = split_glob_parent(&pattern) {
                        if let Ok(entries) = std::fs::read_dir(dir) {
                            for entry in entries.flatten() {
                                let p = entry.path();
                                if let Some(file_name) = p.file_name().and_then(|n| n.to_str()) {
                                    if glob_match(file_pattern, file_name) {
                                        parse_ssh_config_file(&p, seen, hosts);
                                    }
                                }
                            }
                        }
                    }
                } else {
                    let inc_file = std::path::PathBuf::from(pattern);
                    if inc_file.exists() {
                        parse_ssh_config_file(&inc_file, seen, hosts);
                    }
                }
            }
        } else if !current_indices.is_empty() {
            let val = tokens
                .next()
                .map(|v| v.trim_matches('"').trim_matches('\''));
            if let Some(v) = val {
                for &idx in &current_indices {
                    let entry = &mut hosts[idx];
                    match key_lower.as_str() {
                        "hostname" => {
                            if entry.hostname.is_none() {
                                entry.hostname = Some(v.to_string());
                            }
                        }
                        "user" => {
                            if entry.user.is_none() {
                                entry.user = Some(v.to_string());
                            }
                        }
                        "port" => {
                            if entry.port.is_none() {
                                entry.port = v.parse().ok();
                            }
                        }
                        "identityfile" => {
                            if entry.identity_file.is_none() {
                                entry.identity_file = Some(v.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn split_glob_parent(pattern: &str) -> Option<(std::path::PathBuf, &str)> {
    let p = std::path::Path::new(pattern);
    let parent = p.parent()?;
    let file = p.file_name()?.to_str()?;
    Some((parent.to_path_buf(), file))
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pat_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    let mut dp = vec![vec![false; text_chars.len() + 1]; pat_chars.len() + 1];
    dp[0][0] = true;

    for i in 1..=pat_chars.len() {
        if pat_chars[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }

    for i in 1..=pat_chars.len() {
        for j in 1..=text_chars.len() {
            if pat_chars[i - 1] == '*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if pat_chars[i - 1] == '?' || pat_chars[i - 1] == text_chars[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }

    dp[pat_chars.len()][text_chars.len()]
}

/// Checks if ssh-agent has any keys loaded (`ssh-add -l`), returning key comments / identifiers if available
pub fn check_ssh_agent_keys() -> Option<Vec<String>> {
    let output = quarry_core::command("ssh-add")
        .arg("-l")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut keys = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            // ssh-add -l output typically: "256 SHA256:... user@host (ED25519)"
            // Extract comment or show line
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let comment = parts[2..].join(" ");
                keys.push(comment);
            } else {
                keys.push(trimmed.to_string());
            }
        }
    }
    Some(keys)
}
