// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! SSH tunnels (C4, UC2) via the system `ssh` client: agent or key file, no prompts.

use serde::{Deserialize, Serialize};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
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
    /// Forwards a free local port to `remote_host:remote_port` as seen from the SSH server
    pub fn open(cfg: &SshConfig, remote_host: &str, remote_port: u16) -> Result<Tunnel, String> {
        let local_port = TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map_err(|e| e.to_string())?.port();
        let mut cmd = Command::new("ssh");
        cmd.args(["-N", "-L", &format!("127.0.0.1:{local_port}:{remote_host}:{remote_port}")])
            .args(["-p", &cfg.port.to_string()])
            // Never prompt (passphrase keys must be in ssh-agent); fail fast if the forward can't be set up
            .args(["-o", "BatchMode=yes", "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=30"])
            .args(["-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=10"])
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
fn watch(app_pid: u32, ssh_pid: u32) {
    let script = format!("while kill -0 {app_pid} 2>/dev/null && kill -0 {ssh_pid} 2>/dev/null; do sleep 2; done; kill {ssh_pid} 2>/dev/null");
    let _ = Command::new("sh").args(["-c", &script]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or_else(|_| path.to_string()),
        None => path.to_string(),
    }
}

/// Host and port a Postgres connection string points at (defaults: localhost:5432)
pub fn target(conn: &str) -> (String, u16) {
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
pub fn retarget(conn: &str, port: u16) -> String {
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
    fn finds_and_rewrites_the_database_address() {
        let url = "postgres://app:pw@db.internal:6543/prod?sslmode=disable";
        assert_eq!(target(url), ("db.internal".into(), 6543));
        assert_eq!(retarget(url, 40001), "postgres://app:pw@127.0.0.1:40001/prod?sslmode=disable");
        assert_eq!(target("postgres://app@db"), ("db".into(), 5432));
        assert_eq!(target("host=10.0.0.5 port=5433 dbname=x"), ("10.0.0.5".into(), 5433));
        assert_eq!(retarget("host=10.0.0.5 dbname=x", 40002), "dbname=x host=127.0.0.1 port=40002");
    }
}
