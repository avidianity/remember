use rusqlite::params;

use super::{Hub, check_len, check_secret, required};
use crate::error::{Error, Result};
use crate::model::{age, is_task_ref, parse_ref, task_ref};
use crate::views::{Inbox, MessageRow};

/// Unread Messages for agent `?1` whose Current Task is `?2`: addressed to the
/// Agent itself, or to its Current Task by anyone else.
const UNREAD: &str = "FROM messages m
    WHERE m.from_agent_id != ?1
      AND (m.to_agent_id = ?1 OR (m.to_task_id IS NOT NULL AND m.to_task_id = ?2))
      AND NOT EXISTS (SELECT 1 FROM message_reads r WHERE r.message_id = m.id AND r.agent_id = ?1)";

impl Hub {
    /// Sends a Message to an Online Agent (`codex#91bc`) or to a Task (`t7`).
    pub fn send(&self, agent_id: &str, to: &str, text: &str) -> Result<String> {
        let caller = self.caller(agent_id)?;
        let text = required("message", text)?;
        check_len("message", text, self.limits.message_chars)?;
        check_secret(text)?;
        let to = to.trim();
        let now = self.now();
        if is_task_ref(to) {
            let task_id = parse_ref(to, 't', "task")?;
            let status: Option<String> = self
                .query_rows(
                    "SELECT status FROM tasks WHERE id = ?1 AND project_id = ?2",
                    params![task_id, caller.project_id],
                    |r| r.get(0),
                )?
                .pop();
            match status.as_deref() {
                Some("open") => {}
                Some(_) => {
                    return Err(Error::rejected(format!(
                        "task {} is done; reopen it first",
                        task_ref(task_id)
                    )));
                }
                None => {
                    return Err(Error::rejected(format!(
                        "task {} not found in this project",
                        task_ref(task_id)
                    )));
                }
            }
            self.conn.execute(
                "INSERT INTO messages (from_agent_id, to_task_id, text, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![agent_id, task_id, text, now],
            )?;
            return Ok("ok".to_string());
        }
        if to == agent_id {
            return Err(Error::rejected("cannot message yourself; use remember"));
        }
        match self.is_online(to)? {
            Some(true) => {}
            Some(false) => return Err(Error::rejected("agent offline; address the task instead")),
            None => return Err(Error::rejected(format!("agent {to} not found"))),
        }
        self.conn.execute(
            "INSERT INTO messages (from_agent_id, to_agent_id, text, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![agent_id, to, text, now],
        )?;
        Ok("ok".to_string())
    }

    /// Returns unread Messages and marks them read.
    pub fn inbox(&mut self, agent_id: &str) -> Result<Inbox> {
        let caller = self.caller(agent_id)?;
        let now = self.now();
        let tx = self.conn.transaction()?;
        let rows = {
            let mut stmt = tx.prepare_cached(&format!(
                "SELECT m.id, m.from_agent_id, m.to_agent_id, m.to_task_id, m.created_at, m.text {UNREAD} ORDER BY m.id"
            ))?;
            stmt.query_map(params![agent_id, caller.current_task_id], |r| {
                let to_agent: Option<String> = r.get(2)?;
                let to_task: Option<i64> = r.get(3)?;
                Ok((
                    r.get::<_, i64>(0)?,
                    MessageRow {
                        from: r.get(1)?,
                        to: to_task.map(task_ref).or(to_agent).unwrap_or_default(),
                        age: age(now - r.get::<_, i64>(4)?),
                        text: r.get(5)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, _) in &rows {
            tx.execute(
                "INSERT OR IGNORE INTO message_reads (message_id, agent_id, read_at) VALUES (?1, ?2, ?3)",
                params![id, agent_id, now],
            )?;
        }
        tx.commit()?;
        Ok(Inbox {
            messages: rows.into_iter().map(|(_, row)| row).collect(),
        })
    }

    pub fn unread_count(&self, agent_id: &str) -> Result<i64> {
        let caller = self.caller(agent_id)?;
        Ok(self.conn.query_row(
            &format!("SELECT count(*) {UNREAD}"),
            params![agent_id, caller.current_task_id],
            |r| r.get(0),
        )?)
    }
}
