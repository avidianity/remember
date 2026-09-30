//! `remember update`: replaces the running binary with the latest release.
//!
//! The installer is embedded and run against the running binary's directory,
//! so downloading, checksum verification and the atomic swap live in exactly
//! one place, the script users already pipe from the web.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

const INSTALLER: &str = include_str!("../../../install.sh");

pub fn run(force: bool) -> Result<()> {
    if cfg!(windows) {
        bail!(
            "update is not supported on Windows; download the latest .zip from \
             https://github.com/avidianity/remember/releases"
        );
    }
    // Resolve symlinks so the real file is replaced and every link to it
    // picks up the new version.
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("cannot locate the running remember binary")?;
    if exe.file_name().and_then(|n| n.to_str()) != Some("remember") {
        bail!(
            "{} is not named `remember`; reinstall with the install script instead",
            exe.display()
        );
    }
    let dir = exe
        .parent()
        .context("the remember binary has no parent directory")?;

    let mut child = Command::new("sh")
        .arg("-s")
        .env("REMEMBER_INSTALL_DIR", dir)
        .env("REMEMBER_CURRENT_VERSION", env!("CARGO_PKG_VERSION"))
        .env("REMEMBER_FORCE", if force { "1" } else { "0" })
        .stdin(Stdio::piped())
        .spawn()
        .context("cannot run sh")?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(INSTALLER.as_bytes())
        .context("cannot pass the installer to sh")?;
    let status = child.wait().context("the installer did not finish")?;
    if !status.success() {
        bail!("update failed; see the installer output above");
    }
    Ok(())
}
