//! Resolves a directory to its Project identity.
//!
//! Order: normalized `origin` remote URL, else git root path, else the directory
//! itself. Git metadata is read directly from disk so resolution never spawns a
//! process on the startup path.

use std::fs;
use std::path::{Path, PathBuf};

pub fn identify(dir: &Path) -> String {
    let dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let Some((root, git_dir)) = find_git(&dir) else {
        return dir.display().to_string();
    };
    origin_url(&git_dir)
        .and_then(|url| normalize_remote(&url))
        .unwrap_or_else(|| root.display().to_string())
}

/// Walks up from `dir` to the first directory holding `.git`, returning the
/// work tree root and the common git directory (shared by all worktrees).
fn find_git(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    for candidate in dir.ancestors() {
        let dot_git = candidate.join(".git");
        if dot_git.is_dir() {
            return Some((candidate.to_path_buf(), dot_git));
        }
        if dot_git.is_file() {
            let content = fs::read_to_string(&dot_git).ok()?;
            let git_dir = content.trim().strip_prefix("gitdir:")?.trim();
            let git_dir = candidate.join(git_dir);
            let common = match fs::read_to_string(git_dir.join("commondir")) {
                Ok(common) => git_dir.join(common.trim()),
                Err(_) => git_dir,
            };
            return Some((candidate.to_path_buf(), common));
        }
    }
    None
}

fn origin_url(git_dir: &Path) -> Option<String> {
    let config = fs::read_to_string(git_dir.join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line.replace(' ', "") == r#"[remote"origin"]"#;
            continue;
        }
        if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// `git@github.com:Foo/bar.git`, `https://user@GitHub.com/Foo/bar` and
/// `ssh://git@github.com:22/Foo/bar.git` all become `github.com/Foo/bar`.
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        let host = host.split(':').next()?;
        (host, path)
    } else if let Some((authority, path)) = url.split_once(':') {
        if authority.contains('/') {
            return None;
        }
        (authority.rsplit('@').next()?, path)
    } else {
        return None;
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{}", host.to_ascii_lowercase(), path))
}

/// Converts an MCP root URI (`file:///home/me/repo`) to a path.
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16)
        {
            decoded.push(byte);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_remote_forms_to_one_identity() {
        for url in [
            "git@github.com:avidianity/remember.git",
            "https://github.com/avidianity/remember",
            "https://token@GitHub.com/avidianity/remember.git/",
            "ssh://git@github.com:22/avidianity/remember.git",
        ] {
            assert_eq!(
                normalize_remote(url).as_deref(),
                Some("github.com/avidianity/remember"),
                "{url}"
            );
        }
        assert_eq!(normalize_remote("/local/path/repo"), None);
    }

    #[test]
    fn decodes_file_uris() {
        assert_eq!(
            path_from_uri("file:///home/me/my%20repo"),
            Some(PathBuf::from("/home/me/my repo"))
        );
        assert_eq!(path_from_uri("https://example.com"), None);
    }

    #[test]
    fn identifies_by_remote_then_root_then_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = tmp.path().join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(
            identify(&plain),
            fs::canonicalize(&plain).unwrap().display().to_string()
        );

        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("src/deep")).unwrap();
        let root = fs::canonicalize(&repo).unwrap().display().to_string();
        assert_eq!(identify(&repo.join("src/deep")), root);

        fs::write(
            repo.join(".git/config"),
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:avidianity/remember.git\n",
        )
        .unwrap();
        assert_eq!(
            identify(&repo.join("src")),
            "github.com/avidianity/remember"
        );

        // A linked worktree shares the main repository's remote.
        let worktree = tmp.path().join("wt");
        fs::create_dir_all(repo.join(".git/worktrees/wt")).unwrap();
        fs::write(repo.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        assert_eq!(identify(&worktree), "github.com/avidianity/remember");
    }
}
