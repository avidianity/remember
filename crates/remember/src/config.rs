//! Host configuration (DB path, limits) and human-facing Markdown rendering.

use std::fmt::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use remember_core::Limits;
use remember_core::hub::{ExportMemory, MemoryDetail};

/// `REMEMBER_DB` when set, else `$HOME/.local/share/remember/remember.db`.
/// `XDG_DATA_HOME` is deliberately ignored: Codex strips it from the server's
/// environment, which would split one user's memory across two files (ADR 0001).
pub fn db_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("REMEMBER_DB").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            bail!(
                "REMEMBER_DB must be an absolute path, got {}",
                path.display()
            );
        }
        return Ok(path);
    }
    let home = std::env::var_os("HOME").context("HOME is not set; set REMEMBER_DB")?;
    Ok(PathBuf::from(home).join(".local/share/remember/remember.db"))
}

/// Defaults, overridable per variable, e.g. `REMEMBER_MEMORY_CHARS=2000`.
pub fn limits() -> Limits {
    let mut limits = Limits::default();
    let vars: [(&str, &mut usize); 5] = [
        ("REMEMBER_MEMORY_CHARS", &mut limits.memory_chars),
        ("REMEMBER_BRIEF_CHARS", &mut limits.brief_chars),
        ("REMEMBER_CONTEXT_TOKENS", &mut limits.context_tokens),
        ("REMEMBER_RECALL_TOKENS", &mut limits.recall_tokens),
        ("REMEMBER_JOIN_TOKENS", &mut limits.join_tokens),
    ];
    for (name, slot) in vars {
        if let Some(value) = std::env::var(name).ok().and_then(|v| v.parse().ok()) {
            *slot = value;
        }
    }
    limits
}

fn memory_line(out: &mut String, m: &ExportMemory) {
    let place = match (&m.project, &m.task) {
        (Some(project), Some(task)) => format!("{project} {task}"),
        (Some(project), None) => project.clone(),
        _ => "user".to_string(),
    };
    let key = m
        .key
        .as_deref()
        .map(|k| format!(" `{k}`"))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "- **{}** {} {}{key} ({place}, {}, {})\n  {}",
        m.id, m.category, m.scope, m.agent, m.updated, m.text
    );
}

pub fn markdown_memories(memories: &[ExportMemory]) -> String {
    let mut out = String::from("# Memories\n\n");
    memories.iter().for_each(|m| memory_line(&mut out, m));
    out
}

pub fn markdown_detail(detail: &MemoryDetail) -> String {
    let mut out = String::new();
    memory_line(&mut out, &detail.memory);
    if !detail.revisions.is_empty() {
        out.push_str("\n## Revisions\n\n");
        for r in &detail.revisions {
            let _ = writeln!(
                out,
                "- {} {} ({})\n  {}",
                r.written, r.category, r.agent, r.text
            );
        }
    }
    out
}

pub fn markdown_export(export: &remember_core::hub::Export) -> String {
    let mut out = markdown_memories(&export.memories);
    out.push_str("\n# Tasks\n");
    for t in &export.tasks {
        let _ = writeln!(
            out,
            "\n## {} {} ({}, {}, {})\n",
            t.id, t.title, t.project, t.status, t.updated
        );
        if let Some(outcome) = &t.outcome {
            let _ = writeln!(out, "Outcome: {outcome}\n");
        }
        if !t.brief.is_empty() {
            let _ = writeln!(out, "{}", t.brief);
        }
    }
    out
}
