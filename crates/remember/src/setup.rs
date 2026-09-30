//! `remember setup <agent>`: registers the server through each agent's own CLI,
//! so we never hand-edit another tool's config file (opencode's is JSONC).

use std::io::{BufRead, Write};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;

use crate::config;

#[derive(Clone, Copy, ValueEnum)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Opencode,
}

pub fn run(agent: AgentKind, yes: bool) -> Result<()> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("locating the remember binary")?;
    let exe = exe
        .to_str()
        .context("binary path is not UTF-8")?
        .to_string();
    let db = config::db_path()?;
    let db = db.to_str().context("DB path is not UTF-8")?.to_string();
    // The absolute DB path is pinned in each agent's config because agents
    // pass different environments to the servers they spawn.
    let env = format!("REMEMBER_DB={db}");
    let (program, args): (&str, Vec<String>) = match agent {
        AgentKind::ClaudeCode => (
            "claude",
            vec![
                "mcp", "add", "--scope", "user", "remember", "-e", &env, "--", &exe, "serve",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        ),
        AgentKind::Codex => (
            "codex",
            vec!["mcp", "add", "remember", "--env", &env, "--", &exe, "serve"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
        AgentKind::Opencode => (
            "opencode",
            vec!["mcp", "add", "remember", "--env", &env, "--", &exe, "serve"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
    };

    println!("This will run:\n\n  {program} {}\n", shell_join(&args));
    if !yes && !confirm()? {
        println!("aborted");
        return Ok(());
    }
    let status = Command::new(program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .status()
        .with_context(|| format!("running {program}; is it installed and on PATH?"))?;
    if !status.success() {
        bail!("{program} exited with {status}; if remember is already registered, remove it first");
    }
    if let AgentKind::Codex = agent {
        // `codex mcp add` cannot set this, and without it Codex asks before
        // every Remember call (and refuses them outright in `codex exec`).
        println!(
            "\nCodex asks for approval on each MCP call. To allow Remember's tools, add to ~/.codex/config.toml:\n\n  \
             [mcp_servers.remember]\n  default_tools_approval_mode = \"approve\""
        );
    }
    Ok(())
}

fn confirm() -> Result<bool> {
    print!("Proceed? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./=#:".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
