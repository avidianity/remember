//! End-to-end: `remember update` against a local stand-in for GitHub
//! Releases, replacing a copy of the real binary.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Serves `releases/latest` as a redirect to `tag`, and any download as
/// `archive` or its checksum (corrupted when `bad_checksum`).
fn release_server(tag: &'static str, archive: Vec<u8>, bad_checksum: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/releases", listener.local_addr().unwrap());
    let hash = sha256(&archive);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = String::new();
            let mut reader = BufReader::new(&stream);
            reader.read_line(&mut request).unwrap();
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let mut parts = request.split_whitespace();
            let (method, path) = (parts.next().unwrap(), parts.next().unwrap());
            let (status, location, body) = if path == "/releases/latest" {
                (
                    "302 Found",
                    Some(format!("/releases/tag/{tag}")),
                    Vec::new(),
                )
            } else if path.starts_with("/releases/tag/") {
                ("200 OK", None, Vec::new())
            } else if let Some(name) = path.strip_suffix(".sha256") {
                let name = name.rsplit('/').next().unwrap();
                let hash = if bad_checksum {
                    "0".repeat(64)
                } else {
                    hash.clone()
                };
                (
                    "200 OK",
                    None,
                    format!("{hash}  {name}.tar.gz\n").into_bytes(),
                )
            } else if path.ends_with(".tar.gz") {
                ("200 OK", None, archive.clone())
            } else {
                ("404 Not Found", None, Vec::new())
            };
            let mut head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            if let Some(location) = location {
                head.push_str(&format!("Location: {location}\r\n"));
            }
            head.push_str("\r\n");
            stream.write_all(head.as_bytes()).unwrap();
            if method != "HEAD" {
                stream.write_all(&body).unwrap();
            }
        }
    });
    base
}

fn sha256(bytes: &[u8]) -> String {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), bytes).unwrap();
    let out = Command::new("sha256sum")
        .arg(tmp.path())
        .output()
        .or_else(|_| {
            Command::new("shasum")
                .args(["-a", "256"])
                .arg(tmp.path())
                .output()
        })
        .unwrap();
    String::from_utf8(out.stdout).unwrap()[..64].to_string()
}

/// A release archive whose `remember` just reports a new version.
fn fake_release(version: &str) -> Vec<u8> {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("pkg");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("remember"),
        format!("#!/bin/sh\necho 'remember {version}'\n"),
    )
    .unwrap();
    let archive = tmp.path().join("release.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(tmp.path())
        .arg("pkg")
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::read(archive).unwrap()
}

/// A copy of the real binary in its own directory, as if installed there.
fn installed() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("remember");
    std::fs::copy(env!("CARGO_BIN_EXE_remember"), &bin).unwrap();
    (tmp, bin)
}

fn update(bin: &Path, releases: &str, args: &[&str]) -> Output {
    Command::new(bin)
        .arg("update")
        .args(args)
        .env("REMEMBER_RELEASES_URL", releases)
        .env_remove("REMEMBER_VERSION")
        .output()
        .unwrap()
}

fn version(bin: &Path) -> String {
    let out = Command::new(bin).arg("--version").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn update_replaces_the_binary_with_a_newer_release() {
    let (_tmp, bin) = installed();
    let releases = release_server("v9.9.9", fake_release("9.9.9"), false);
    let out = update(&bin, &releases, &[]);
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{log}");
    assert!(log.contains("Checksum OK"), "{log}");
    assert!(
        log.contains(&format!(
            "Updated remember v{} to v9.9.9.",
            env!("CARGO_PKG_VERSION")
        )),
        "{log}"
    );
    assert_eq!(version(&bin), "remember 9.9.9");
    let leftovers: Vec<_> = std::fs::read_dir(bin.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["remember"], "staging file left behind");
}

#[test]
fn upgrade_keeps_the_binary_when_already_current() {
    let (_tmp, bin) = installed();
    let releases = release_server("v0.0.1", fake_release("0.0.1"), false);
    let out = update(&bin, &releases, &[]);
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{log}");
    assert!(
        log.contains("is up to date (latest release: v0.0.1)"),
        "{log}"
    );
    let current = format!("remember {}", env!("CARGO_PKG_VERSION"));
    assert_eq!(version(&bin), current, "never downgrades");

    let forced = Command::new(&bin)
        .args(["upgrade", "--force"])
        .env("REMEMBER_RELEASES_URL", &releases)
        .output()
        .unwrap();
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert_eq!(
        version(&bin),
        "remember 0.0.1",
        "--force reinstalls the latest"
    );
}

#[test]
fn update_refuses_a_download_that_fails_its_checksum() {
    let (_tmp, bin) = installed();
    let releases = release_server("v9.9.9", fake_release("9.9.9"), true);
    let out = update(&bin, &releases, &[]);
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{log}");
    assert!(log.contains("checksum verification failed"), "{log}");
    assert_eq!(
        version(&bin),
        format!("remember {}", env!("CARGO_PKG_VERSION"))
    );
}
