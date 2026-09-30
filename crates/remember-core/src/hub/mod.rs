//! The transport-agnostic core: every operation an Agent or the human CLI can perform.

mod admin;
mod agent;
mod memory;
mod message;
mod task;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db;
use crate::error::{Error, Result};
use crate::model::Limits;

pub use admin::{Export, ExportMemory, ExportRevision, ExportTask, MemoryDetail};
pub use agent::Terminal;
pub use memory::{NewMemory, RecallQuery};

type Clock = Box<dyn Fn() -> i64 + Send>;

pub struct Hub {
    conn: Connection,
    limits: Limits,
    clock: Clock,
}

/// The facts about the calling Agent that most operations need.
struct Caller {
    project_id: i64,
    current_task_id: Option<i64>,
}

impl Hub {
    pub fn open(path: &Path, limits: Limits) -> Result<Self> {
        Ok(Self::new(db::open(path)?, limits))
    }

    pub fn open_in_memory(limits: Limits) -> Result<Self> {
        Ok(Self::new(db::open_in_memory()?, limits))
    }

    fn new(conn: Connection, limits: Limits) -> Self {
        Hub {
            conn,
            limits,
            clock: Box::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            }),
        }
    }

    /// Replaces the wall clock; tests use this to move time.
    pub fn with_clock(mut self, clock: impl Fn() -> i64 + Send + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// Checks the schema is still one this binary understands; another Agent's
    /// newer binary may have migrated the shared file since this one opened it.
    pub fn ensure_supported(&self) -> Result<()> {
        db::ensure_supported(&self.conn)
    }

    fn caller(&self, agent_id: &str) -> Result<Caller> {
        self.conn
            .query_row(
                "SELECT project_id, current_task_id FROM agents WHERE id = ?1",
                params![agent_id],
                |r| {
                    Ok(Caller {
                        project_id: r.get(0)?,
                        current_task_id: r.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::rejected(format!("agent {agent_id} is not registered")))
    }

    fn project_ident(&self, project_id: i64) -> Result<String> {
        Ok(self.conn.query_row(
            "SELECT ident FROM projects WHERE id = ?1",
            params![project_id],
            |r| r.get(0),
        )?)
    }

    /// Drops what retention rules say nobody will need again. Runs at server startup.
    pub fn prune(&self) -> Result<()> {
        let now = self.now();
        let week = now - 7 * crate::model::DAY;
        let month = now - 30 * crate::model::DAY;
        let offline = now - self.limits.online_window_secs;
        self.conn.execute(
            "DELETE FROM messages WHERE to_agent_id IS NOT NULL AND (
                id IN (SELECT message_id FROM message_reads WHERE read_at < ?1)
                OR (created_at < ?1 AND to_agent_id NOT IN (
                    SELECT id FROM agents WHERE ended_at IS NULL AND last_seen_at >= ?2)))",
            params![week, offline],
        )?;
        self.conn.execute(
            "DELETE FROM messages WHERE to_task_id IN (
                SELECT id FROM tasks WHERE status = 'done' AND closed_at < ?1)",
            params![month],
        )?;
        self.conn.execute(
            "DELETE FROM agents WHERE coalesce(ended_at, last_seen_at) < ?1",
            params![month],
        )?;
        Ok(())
    }
}

fn check_len(what: &str, text: &str, max: usize) -> Result<()> {
    let len = text.chars().count();
    if len > max {
        return Err(Error::rejected(format!(
            "{what} too long ({len}/{max}); split or summarize"
        )));
    }
    Ok(())
}

fn check_secret(text: &str) -> Result<()> {
    match crate::secrets::detect(text) {
        Some(kind) => Err(Error::rejected(format!(
            "looks like a secret ({kind}); store a reference, not the value"
        ))),
        None => Ok(()),
    }
}

fn required<'a>(what: &str, text: &'a str) -> Result<&'a str> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::rejected(format!("{what} is empty")));
    }
    Ok(text)
}

#[cfg(test)]
mod tests;
