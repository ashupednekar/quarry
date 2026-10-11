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
        let local_port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| e.to_string())?
            .port();
        let mut cmd = Command::new("ssh");
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
fn watch(app_pid: u32, ssh_pid: u32) {
    let script = format!("while kill -0 {app_pid} 2>/dev/null && kill -0 {ssh_pid} 2>/dev/null; do sleep 2; done; kill {ssh_pid} 2>/dev/null");
    let _ = Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var("HOME")
            .map(|h| format!("{h}/{rest}"))
            .unwrap_or_else(|_| path.to_string()),
        None => path.to_string(),
    }
}
