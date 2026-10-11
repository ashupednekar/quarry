// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Oracle Instant Client on demand: one pinned, checksummed Basic Lite build per platform,
//! downloaded only when the user asks and unpacked into Quarry's data folder.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LICENCE_URL: &str = "https://www.oracle.com/downloads/licenses/instant-client-lic.html";

pub struct Package {
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_mb: u32,
}

/// The tested build for this platform. Bumping a version means a new url and checksum here.
pub fn package() -> Option<&'static Package> {
    const MAC_ARM: Package = Package {
        version: "23.26.2",
        url: "https://download.oracle.com/otn_software/mac/instantclient/2326200/instantclient-basiclite-macos.arm64-23.26.2.0.0.dmg",
        sha256: "54defa9e957da0aef6219965da3cb2d22ac19bfb20f168741b424c89e90d47ea",
        size_mb: 66,
    };
    // Oracle's newest for Intel Macs
    const MAC_X64: Package = Package {
        version: "19.16",
        url: "https://download.oracle.com/otn_software/mac/instantclient/1916000/instantclient-basiclite-macos.x64-19.16.0.0.0dbru.dmg",
        sha256: "6b5df6b0c9fe704990679609a50313f2123cae36171dafc954825879b8391eb0",
        size_mb: 41,
    };
    const LINUX_X64: Package = Package {
        version: "23.26.3",
        url: "https://download.oracle.com/otn_software/linux/instantclient/2326300/instantclient-basiclite-linux.x64-23.26.3.0.0.zip",
        sha256: "88c2b59d473f0d34615556dbee51cb90cce61a9104b4c1c772d13c344fba19c9",
        size_mb: 76,
    };
    const LINUX_ARM: Package = Package {
        version: "23.26.3",
        url: "https://download.oracle.com/otn_software/linux/instantclient/2326300/instantclient-basiclite-linux.arm64-23.26.3.0.0.zip",
        sha256: "6559d63a81133e821cbf7fd81d023827813f3262a1ea45a32d364eeeccc68579",
        size_mb: 68,
    };
    const WIN_X64: Package = Package {
        version: "23.26.3",
        url: "https://download.oracle.com/otn_software/nt/instantclient/2326300/instantclient-basiclite-windows.x64-23.26.3.0.0.zip",
        sha256: "9d9b9a8deafbc94086789d4b16c8156deb6a101be9db1a52d3d7c6b714a0fa07",
        size_mb: 64,
    };
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some(&MAC_ARM),
        ("macos", "x86_64") => Some(&MAC_X64),
        ("linux", "x86_64") => Some(&LINUX_X64),
        ("linux", "aarch64") => Some(&LINUX_ARM),
        ("windows", "x86_64") => Some(&WIN_X64),
        _ => None,
    }
}

/// Where Quarry keeps its own copy: the platform's per-user data folder
fn data_dir() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = match std::env::consts::OS {
        "macos" => home()?.join("Library/Application Support"),
        "windows" => std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?,
        _ => std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| Some(home()?.join(".local/share")))?,
    };
    Some(base.join("quarry").join("instantclient"))
}

/// The library ODPI-C loads, by platform
pub fn has_client_lib(dir: &Path) -> bool {
    ["libclntsh.dylib", "libclntsh.so", "oci.dll"].iter().any(|f| dir.join(f).exists())
}

/// Quarry's downloaded client, if installed
pub fn installed() -> Option<PathBuf> {
    let dir = data_dir()?.join(package()?.version);
    has_client_lib(&dir).then_some(dir)
}

/// Downloads, verifies and unpacks the pinned client. Blocking; returns the library folder.
pub fn install() -> Result<PathBuf, String> {
    let pkg = package().ok_or("Quarry has no Oracle client download for this platform")?;
    let root = data_dir().ok_or("Couldn't find a folder to install into")?;
    let dest = root.join(pkg.version);
    let work = root.join(format!("{}.partial", pkg.version));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let archive = work.join(pkg.url.rsplit('/').next().unwrap_or("client"));
        run(Command::new("curl").args(["-fsSL", "--retry", "2", "-o"]).arg(&archive).arg(pkg.url), "download")?;
        verify(&archive, pkg.sha256)?;
        let unpacked = work.join("unpacked");
        std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
        unpack(&archive, &unpacked)?;
        // Zips hold one instantclient_* folder; the dmg holds the files directly
        let lib = find_lib_dir(&unpacked).ok_or("The download didn't contain the Oracle client library")?;
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&lib, &dest).map_err(|e| e.to_string())?;
        Ok(dest.clone())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn verify(file: &Path, expected: &str) -> Result<(), String> {
    let mut hasher = Sha256::new();
    std::io::copy(&mut std::fs::File::open(file).map_err(|e| e.to_string())?, &mut hasher).map_err(|e| e.to_string())?;
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got == expected {
        Ok(())
    } else {
        Err("The downloaded Oracle client doesn't match its checksum, so it wasn't installed".into())
    }
}

fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    match std::env::consts::OS {
        "macos" => {
            let mnt = into.with_file_name("mnt");
            run(Command::new("hdiutil").args(["attach", "-quiet", "-nobrowse", "-readonly", "-mountpoint"]).arg(&mnt).arg(archive), "open the disk image")?;
            let copied = run(Command::new("cp").arg("-R").arg(format!("{}/.", mnt.display())).arg(into), "copy the client");
            let _ = Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mnt).status();
            copied
        }
        // bsdtar ships with Windows 10+ and reads zips
        "windows" => run(Command::new("tar").arg("-xf").arg(archive).arg("-C").arg(into), "unzip the client"),
        _ => run(Command::new("unzip").arg("-q").arg(archive).arg("-d").arg(into), "unzip the client (is unzip installed?)"),
    }
}

fn find_lib_dir(dir: &Path) -> Option<PathBuf> {
    if has_client_lib(dir) {
        return Some(dir.to_path_buf());
    }
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).find(|p| has_client_lib(p))
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!("Couldn't {what}: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("Couldn't {what}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_the_download_against_its_checksum() {
        let f = std::env::temp_dir().join(format!("quarry-ic-{}", std::process::id()));
        std::fs::write(&f, b"not the client").unwrap();
        assert!(verify(&f, package().map_or("00", |p| p.sha256)).is_err());
        assert!(verify(&f, "7918e0d07dbc6b3b0c8ae2561b5d164026fe9287bb014e98f0b7c4b145b41447").is_ok());
        std::fs::remove_file(f).unwrap();
    }
}
