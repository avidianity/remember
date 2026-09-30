use std::hash::{BuildHasher, Hasher};

use rusqlite::{OptionalExtension, params};

use super::{Hub, check_len};
use crate::error::Result;
use crate::model::{age, memory_ref, task_ref};
use crate::views::{AgentRow, Context, DecisionRow, TaskRow, UserRow, fit, row_tokens};

/// Where an Agent runs, so it can be woken by typing into its terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    /// The Agent's own process (the parent of its Remember server).
    pub pid: i64,
    pub tmux_socket: Option<String>,
    pub tmux_pane: Option<String>,
}

impl Hub {
    /// Records a newly connected Agent and returns its id, e.g. `claude-code#3f2a`.
    pub fn register(&self, kind: &str, project_ident: &str) -> Result<String> {
        let kind = normalize_kind(kind);
        let now = self.now();
        self.conn.execute(
            "INSERT INTO projects (ident, created_at) VALUES (?1, ?2) ON CONFLICT (ident) DO NOTHING",
            params![project_ident, now],
        )?;
        let project_id: i64 = self.conn.query_row(
            "SELECT id FROM projects WHERE ident = ?1",
            params![project_ident],
            |r| r.get(0),
        )?;
        let mut width = 4;
        loop {
            for _ in 0..8 {
                let id = format!("{kind}#{}", random_hex(width));
                let inserted = self.conn.execute(
                    "INSERT INTO agents (id, kind, project_id, started_at, last_seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?4) ON CONFLICT (id) DO NOTHING",
                    params![id, kind, project_id, now],
                )?;
                if inserted == 1 {
                    return Ok(id);
                }
            }
            width += 2;
        }
    }

    pub fn set_terminal(&self, agent_id: &str, terminal: &Terminal) -> Result<()> {
        self.conn.execute(
            "UPDATE agents SET pid = ?2, tmux_socket = ?3, tmux_pane = ?4 WHERE id = ?1",
            params![
                agent_id,
                terminal.pid,
                terminal.tmux_socket,
                terminal.tmux_pane
            ],
        )?;
        Ok(())
    }

    /// Terminals of the online Agents a Message to `to` reaches (an Agent id or
    /// a Task), excluding the sender and Agents not running inside tmux.
    pub fn wake_targets(&self, agent_id: &str, to: &str) -> Result<Vec<Terminal>> {
        let caller = self.caller(agent_id)?;
        let online_since = self.now() - self.limits.online_window_secs;
        let (filter, target) = if crate::model::is_task_ref(to) {
            (
                "current_task_id = ?3 AND project_id = ?4",
                rusqlite::types::Value::Integer(crate::model::parse_ref(to, 't', "task")?),
            )
        } else {
            (
                "id = ?3 AND project_id = ?4",
                rusqlite::types::Value::Text(to.trim().into()),
            )
        };
        self.query_rows(
            &format!(
                "SELECT pid, tmux_socket, tmux_pane FROM agents
                 WHERE {filter} AND id != ?1 AND ended_at IS NULL AND last_seen_at >= ?2
                   AND pid IS NOT NULL AND tmux_pane IS NOT NULL"
            ),
            params![agent_id, online_since, target, caller.project_id],
            |r| {
                Ok(Terminal {
                    pid: r.get(0)?,
                    tmux_socket: r.get(1)?,
                    tmux_pane: r.get(2)?,
                })
            },
        )
    }

    /// Marks the Agent as seen; called on every tool call.
    pub fn touch(&self, agent_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE agents SET last_seen_at = ?2 WHERE id = ?1",
            params![agent_id, self.now()],
        )?;
        Ok(())
    }

    /// Marks the Agent offline; called when its process exits cleanly.
    pub fn end(&self, agent_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE agents SET ended_at = ?2, current_task_id = NULL WHERE id = ?1",
            params![agent_id, self.now()],
        )?;
        Ok(())
    }

    /// The payload an Agent reads first: who it is, User Memories, Project
    /// decisions and open Tasks, cut to the context token budget.
    pub fn context(&self, agent_id: &str, label: Option<&str>) -> Result<Context> {
        let caller = self.caller(agent_id)?;
        if let Some(label) = label.map(str::trim).filter(|l| !l.is_empty()) {
            check_len("label", label, 40)?;
            self.conn.execute(
                "UPDATE agents SET label = ?2 WHERE id = ?1",
                params![agent_id, label],
            )?;
        }
        let now = self.now();
        let project = self.project_ident(caller.project_id)?;
        // No row costs under 4 tokens, so the budget can never hold more rows
        // than this; fetching one extra shows whether anything was left out.
        let cap = (self.limits.context_tokens / 4) as i64;
        // The IS NULL terms below are implied by scope, but they let SQLite walk
        // the bucket index in updated_at order and stop at the LIMIT.
        let mut user = self.query_rows(
            "SELECT id, key, text FROM memories
             WHERE scope = 'user' AND project_id IS NULL AND task_id IS NULL
             ORDER BY updated_at DESC, id DESC LIMIT ?1",
            params![cap + 1],
            |r| {
                Ok(UserRow {
                    id: memory_ref(r.get(0)?),
                    key: r.get(1)?,
                    text: r.get(2)?,
                })
            },
        )?;
        let mut decisions = self.query_rows(
            "SELECT id, key, updated_at, text FROM memories
             WHERE scope = 'project' AND project_id = ?1 AND task_id IS NULL AND category = 'decision'
             ORDER BY updated_at DESC, id DESC LIMIT ?2",
            params![caller.project_id, cap + 1],
            |r| {
                Ok(DecisionRow {
                    id: memory_ref(r.get(0)?),
                    key: r.get(1)?,
                    age: age(now - r.get::<_, i64>(2)?),
                    text: r.get(3)?,
                })
            },
        )?;
        let mut agents = self.agent_rows(agent_id, caller.project_id, now)?;
        agents.truncate(cap as usize + 1);
        let mut tasks = self.open_tasks(caller.project_id, now)?;
        tasks.truncate(cap as usize + 1);
        let capped = |fetched: usize| if fetched as i64 > cap { "+" } else { "" };
        let (user_fetched, decisions_fetched, tasks_fetched, agents_fetched) =
            (user.len(), decisions.len(), tasks.len(), agents.len());

        let header = format!("agent: {agent_id}\nproject: {project}\n");
        let mut budget = self
            .limits
            .context_tokens
            .saturating_sub(row_tokens(&[&header]) + 40);
        let dropped_agents = fit(&mut agents, &mut budget, |a| {
            row_tokens(&[
                &a.id,
                a.label.as_deref().unwrap_or("null"),
                a.task.as_deref().unwrap_or("null"),
                &a.seen,
            ])
        });
        let dropped_tasks = fit(&mut tasks, &mut budget, |t| {
            row_tokens(&[&t.id, &t.title, &t.status, &t.last_agent, &t.age])
        });
        let dropped_user = fit(&mut user, &mut budget, |u| {
            row_tokens(&[&u.id, u.key.as_deref().unwrap_or("null"), &u.text])
        });
        let dropped_decisions = fit(&mut decisions, &mut budget, |d| {
            row_tokens(&[&d.id, d.key.as_deref().unwrap_or("null"), &d.age, &d.text])
        });
        let truncated = [
            (dropped_user, capped(user_fetched), "user memories"),
            (dropped_decisions, capped(decisions_fetched), "decisions"),
            (dropped_tasks, capped(tasks_fetched), "tasks"),
            (dropped_agents, capped(agents_fetched), "agents"),
        ]
        .iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, plus, what)| format!("{n}{plus} more {what}"))
        .collect::<Vec<_>>();

        Ok(Context {
            agent: agent_id.to_string(),
            project,
            current_task: caller.current_task_id.map(task_ref),
            user,
            decisions,
            agents,
            tasks,
            truncated: (!truncated.is_empty())
                .then(|| format!("{}; use recall or task list", truncated.join(", "))),
        })
    }

    pub(super) fn open_tasks(&self, project_id: i64, now: i64) -> Result<Vec<TaskRow>> {
        self.task_rows(project_id, now, false)
    }

    pub(super) fn task_rows(
        &self,
        project_id: i64,
        now: i64,
        include_done: bool,
    ) -> Result<Vec<TaskRow>> {
        let stale_before = now - self.limits.stale_after_secs;
        self.query_rows(
            "SELECT id, title, status, updated_at, last_agent_id FROM tasks
             WHERE project_id = ?1 AND (status = 'open' OR ?2)
             ORDER BY status DESC, updated_at DESC",
            params![project_id, include_done],
            |r| {
                let updated_at: i64 = r.get(3)?;
                let status: String = r.get(2)?;
                Ok(TaskRow {
                    id: task_ref(r.get(0)?),
                    title: r.get(1)?,
                    status: if status == "open" && updated_at < stale_before {
                        "stale".to_string()
                    } else {
                        status
                    },
                    last_agent: r.get(4)?,
                    age: age(now - updated_at),
                })
            },
        )
    }

    pub(super) fn query_rows<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt
            .query_map(params, map)?
            .collect::<rusqlite::Result<Vec<T>>>()?;
        Ok(rows)
    }

    /// An Agent can receive Messages only while its process runs and it has
    /// called a tool within the online window.
    pub(super) fn is_online(&self, agent_id: &str) -> Result<Option<bool>> {
        let row: Option<(Option<i64>, i64)> = self
            .conn
            .query_row(
                "SELECT ended_at, last_seen_at FROM agents WHERE id = ?1",
                params![agent_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(ended, seen)| {
            ended.is_none() && seen >= self.now() - self.limits.online_window_secs
        }))
    }

    /// Other Online Agents in the Project with their label and Current Task,
    /// most recently active first.
    pub(super) fn agent_rows(
        &self,
        agent_id: &str,
        project_id: i64,
        now: i64,
    ) -> Result<Vec<AgentRow>> {
        self.query_rows(
            "SELECT id, label, current_task_id, last_seen_at FROM agents
             WHERE project_id = ?1 AND id != ?2 AND ended_at IS NULL AND last_seen_at >= ?3
             ORDER BY last_seen_at DESC",
            params![project_id, agent_id, now - self.limits.online_window_secs],
            |r| {
                Ok(AgentRow {
                    id: r.get(0)?,
                    label: r.get(1)?,
                    task: r.get::<_, Option<i64>>(2)?.map(task_ref),
                    seen: age(now - r.get::<_, i64>(3)?),
                })
            },
        )
    }
}

/// Lowercase, `[a-z0-9-]` only, so ids stay readable and unambiguous.
fn normalize_kind(kind: &str) -> String {
    let cleaned: String = kind
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        "agent".to_string()
    } else {
        cleaned.chars().take(32).collect()
    }
}

fn random_hex(width: usize) -> String {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    hasher.write_u32(std::process::id());
    let value = hasher.finish();
    format!("{value:016x}")[..width].to_string()
}

#[cfg(test)]
mod tests {
    use super::normalize_kind;

    #[test]
    fn normalizes_kind() {
        assert_eq!(normalize_kind("Claude Code"), "claude-code");
        assert_eq!(normalize_kind("  "), "agent");
        assert_eq!(normalize_kind("open_code!"), "open-code");
    }
}
