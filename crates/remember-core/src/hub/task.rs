use rusqlite::{OptionalExtension, params};

use super::memory::{memory_row_tokens, replace, touch_task};
use super::{Caller, Hub, check_len, check_secret, required};
use crate::error::{Error, Result};
use crate::model::{age, memory_ref, parse_ref, task_ref};
use crate::views::{Brief, MemoryRow, TaskList, TaskListRow, TaskView, fit, row_tokens};

struct TaskRecord {
    id: i64,
    project_id: i64,
    title: String,
    status: String,
    brief: String,
    brief_revision: i64,
}

impl Hub {
    pub fn list_tasks(&self, agent_id: &str, include_done: bool) -> Result<TaskList> {
        let caller = self.caller(agent_id)?;
        let now = self.now();
        let online_since = now - self.limits.online_window_secs;
        let tasks = self
            .task_rows(caller.project_id, now, include_done)?
            .into_iter()
            .map(|t| {
                let task_id = parse_ref(&t.id, 't', "task")?;
                let agents: Vec<String> = self.query_rows(
                    "SELECT id FROM agents WHERE current_task_id = ?1 AND ended_at IS NULL
                     AND last_seen_at >= ?2 ORDER BY last_seen_at DESC",
                    params![task_id, online_since],
                    |r| r.get(0),
                )?;
                Ok(TaskListRow {
                    id: t.id,
                    title: t.title,
                    status: t.status,
                    agents: agents.join(" "),
                    last_agent: t.last_agent,
                    age: t.age,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(TaskList { tasks })
    }

    /// Creates a Task in the caller's Project and makes it the Current Task.
    pub fn start_task(
        &mut self,
        agent_id: &str,
        title: &str,
        brief: Option<&str>,
    ) -> Result<TaskView> {
        let caller = self.caller(agent_id)?;
        let title = required("title", title)?;
        check_len("title", title, 120)?;
        let brief = brief.map(str::trim).unwrap_or("");
        check_len("brief", brief, self.limits.brief_chars)?;
        check_secret(title)?;
        check_secret(brief)?;
        let now = self.now();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO tasks (project_id, title, status, brief, brief_revision, created_by, last_agent_id, created_at, updated_at)
             VALUES (?1, ?2, 'open', ?3, ?4, ?5, ?5, ?6, ?6)",
            params![caller.project_id, title, brief, i64::from(!brief.is_empty()), agent_id, now],
        )?;
        let task_id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE agents SET current_task_id = ?2 WHERE id = ?1",
            params![agent_id, task_id],
        )?;
        tx.commit()?;
        self.task_view(task_id)
    }

    /// Makes an Open Task the Current Task (leaving any previous one) and
    /// returns its Brief plus its latest Memories.
    pub fn join_task(&mut self, agent_id: &str, task: &str) -> Result<TaskView> {
        let caller = self.caller(agent_id)?;
        let record = self.task_in_project(&caller, task)?;
        if record.status == "done" {
            return Err(Error::rejected(format!(
                "task {} is done; reopen it first",
                task_ref(record.id)
            )));
        }
        let now = self.now();
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE agents SET current_task_id = ?2 WHERE id = ?1",
            params![agent_id, record.id],
        )?;
        touch_task(&tx, record.id, agent_id, now)?;
        tx.commit()?;
        self.task_view(record.id)
    }

    pub fn leave_task(&self, agent_id: &str) -> Result<String> {
        let caller = self.caller(agent_id)?;
        if caller.current_task_id.is_none() {
            return Err(Error::rejected("no current task"));
        }
        self.conn.execute(
            "UPDATE agents SET current_task_id = NULL WHERE id = ?1",
            params![agent_id],
        )?;
        Ok("ok".to_string())
    }

    /// Marks a Task done with a one-line outcome, promoting the chosen Task
    /// Memories to Project Scope. Defaults to the Current Task.
    pub fn close_task(
        &mut self,
        agent_id: &str,
        task: Option<&str>,
        outcome: &str,
        promote: &[String],
    ) -> Result<String> {
        let caller = self.caller(agent_id)?;
        let record = match task {
            Some(task) => self.task_in_project(&caller, task)?,
            None => {
                let id = caller
                    .current_task_id
                    .ok_or_else(|| Error::rejected("no current task; pass task"))?;
                self.task_by_id(id)?
            }
        };
        if record.status == "done" {
            return Err(Error::rejected(format!(
                "task {} is already done",
                task_ref(record.id)
            )));
        }
        let outcome = required("outcome", outcome)?;
        check_len("outcome", outcome, 200)?;
        check_secret(outcome)?;
        let promote = promote
            .iter()
            .map(|id| parse_ref(id, 'm', "memory"))
            .collect::<Result<Vec<_>>>()?;
        let now = self.now();
        let keep = self.limits.revisions_kept;

        let tx = self.conn.transaction()?;
        for &memory_id in &promote {
            let found: Option<(Option<String>, String, String)> = tx
                .query_row(
                    "SELECT key, category, text FROM memories WHERE id = ?1 AND scope = 'task' AND task_id = ?2",
                    params![memory_id, record.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((key, category, text)) = found else {
                return Err(Error::rejected(format!(
                    "{} is not a memory of task {}",
                    memory_ref(memory_id),
                    task_ref(record.id)
                )));
            };
            let clash: Option<i64> = match &key {
                Some(key) => tx
                    .query_row(
                        "SELECT id FROM memories WHERE scope = 'project' AND project_id = ?1 AND key = ?2",
                        params![record.project_id, key],
                        |r| r.get(0),
                    )
                    .optional()?,
                None => None,
            };
            match clash {
                // The Project already has this Key: the promoted value supersedes it.
                Some(project_memory) => {
                    replace(&tx, project_memory, &category, &text, agent_id, now, keep)?;
                    tx.execute("DELETE FROM memories WHERE id = ?1", params![memory_id])?;
                }
                None => {
                    tx.execute(
                        "UPDATE memories SET scope = 'project', task_id = NULL, updated_at = ?2 WHERE id = ?1",
                        params![memory_id, now],
                    )?;
                }
            }
        }
        tx.execute(
            "UPDATE tasks SET status = 'done', outcome = ?2, closed_at = ?3, updated_at = ?3, last_agent_id = ?4 WHERE id = ?1",
            params![record.id, outcome, now, agent_id],
        )?;
        tx.execute(
            "UPDATE agents SET current_task_id = NULL WHERE current_task_id = ?1",
            params![record.id],
        )?;
        tx.commit()?;
        Ok(match promote.len() {
            0 => format!("{} done", task_ref(record.id)),
            n => format!("{} done, promoted {n}", task_ref(record.id)),
        })
    }

    /// Reopens a Done Task and joins it.
    pub fn reopen_task(&mut self, agent_id: &str, task: &str) -> Result<TaskView> {
        let caller = self.caller(agent_id)?;
        let record = self.task_in_project(&caller, task)?;
        if record.status == "open" {
            return Err(Error::rejected(format!(
                "task {} is already open",
                task_ref(record.id)
            )));
        }
        self.conn.execute(
            "UPDATE tasks SET status = 'open', closed_at = NULL WHERE id = ?1",
            params![record.id],
        )?;
        self.join_task(agent_id, task)
    }

    /// Returns the Current Task's Brief, or replaces it when `text` is given.
    /// A write must carry the revision it was based on; a stale write is
    /// rejected with the current Brief so no update is silently lost.
    pub fn brief(
        &mut self,
        agent_id: &str,
        text: Option<&str>,
        base_revision: Option<i64>,
    ) -> Result<Brief> {
        let caller = self.caller(agent_id)?;
        let task_id = caller
            .current_task_id
            .ok_or_else(|| Error::rejected("no current task; join or start one"))?;
        let record = self.task_by_id(task_id)?;
        let Some(text) = text else {
            return Ok(Brief {
                task: task_ref(task_id),
                brief_revision: record.brief_revision,
                brief: record.brief,
            });
        };
        let text = text.trim();
        check_len("brief", text, self.limits.brief_chars)?;
        check_secret(text)?;
        let base = base_revision.ok_or_else(|| {
            Error::rejected(format!(
                "base_revision required; current is {}",
                record.brief_revision
            ))
        })?;
        let now = self.now();
        let tx = self.conn.transaction()?;
        let updated = tx.execute(
            "UPDATE tasks SET brief = ?2, brief_revision = brief_revision + 1 WHERE id = ?1 AND brief_revision = ?3",
            params![task_id, text, base],
        )?;
        if updated == 0 {
            drop(tx);
            let current = self.task_by_id(task_id)?;
            let payload = crate::toon::encode(&Brief {
                task: task_ref(task_id),
                brief_revision: current.brief_revision,
                brief: current.brief,
            })?;
            return Err(Error::rejected(format!(
                "brief changed since revision {base}; merge into the current brief and retry\n{payload}"
            )));
        }
        touch_task(&tx, task_id, agent_id, now)?;
        tx.commit()?;
        Ok(Brief {
            task: task_ref(task_id),
            brief_revision: base + 1,
            brief: text.to_string(),
        })
    }

    fn task_view(&self, task_id: i64) -> Result<TaskView> {
        let record = self.task_by_id(task_id)?;
        let now = self.now();
        let mut memories = self.query_rows(
            "SELECT id, category, key, updated_at, text FROM memories
             WHERE scope = 'task' AND task_id = ?1 ORDER BY updated_at DESC, id DESC LIMIT ?2",
            params![task_id, (self.limits.join_tokens / 4) as i64 + 1],
            |r| {
                Ok(MemoryRow {
                    id: memory_ref(r.get(0)?),
                    scope: task_ref(task_id),
                    category: r.get(1)?,
                    key: r.get(2)?,
                    age: age(now - r.get::<_, i64>(3)?),
                    text: r.get(4)?,
                })
            },
        )?;
        let mut budget = self
            .limits
            .join_tokens
            .saturating_sub(row_tokens(&[&record.title, &record.brief]) + 30);
        let dropped = fit(&mut memories, &mut budget, memory_row_tokens);
        Ok(TaskView {
            task: task_ref(record.id),
            title: record.title,
            status: record.status,
            brief_revision: record.brief_revision,
            brief: record.brief,
            memories,
            truncated: (dropped > 0)
                .then(|| format!("{dropped} more memories; use recall scope=task")),
        })
    }

    fn task_in_project(&self, caller: &Caller, task: &str) -> Result<TaskRecord> {
        let id = parse_ref(task, 't', "task")?;
        match self.task_by_id(id) {
            Ok(record) if record.project_id == caller.project_id => Ok(record),
            _ => Err(Error::rejected(format!(
                "task {} not found in this project",
                task_ref(id)
            ))),
        }
    }

    fn task_by_id(&self, id: i64) -> Result<TaskRecord> {
        self.conn
            .query_row(
                "SELECT id, project_id, title, status, brief, brief_revision FROM tasks WHERE id = ?1",
                params![id],
                |r| {
                    Ok(TaskRecord {
                        id: r.get(0)?,
                        project_id: r.get(1)?,
                        title: r.get(2)?,
                        status: r.get(3)?,
                        brief: r.get(4)?,
                        brief_revision: r.get(5)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::rejected(format!("task {} not found", task_ref(id))))
    }
}
