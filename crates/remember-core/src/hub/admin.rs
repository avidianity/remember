//! Operations for the human CLI. They act as the DB owner, not as an Agent,
//! so they see every Scope in every Project.

use rusqlite::types::Value;
use rusqlite::{OptionalExtension, params, params_from_iter};
use serde::Serialize;

use super::Hub;
use crate::error::{Error, Result};
use crate::model::{Scope, iso8601, memory_ref, parse_ref, task_ref};

#[derive(Debug, Serialize)]
pub struct ExportMemory {
    pub id: String,
    pub scope: String,
    pub project: Option<String>,
    pub task: Option<String>,
    pub category: String,
    pub key: Option<String>,
    pub agent: String,
    pub updated: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct ExportRevision {
    pub agent: String,
    pub written: String,
    pub category: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct MemoryDetail {
    pub memory: ExportMemory,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<ExportRevision>,
}

#[derive(Debug, Serialize)]
pub struct ExportTask {
    pub id: String,
    pub project: String,
    pub title: String,
    pub status: String,
    pub outcome: Option<String>,
    pub updated: String,
    pub brief: String,
}

#[derive(Debug, Serialize)]
pub struct Export {
    pub memories: Vec<ExportMemory>,
    pub tasks: Vec<ExportTask>,
}

const MEMORY_COLUMNS: &str =
    "SELECT m.id, m.scope, p.ident, m.task_id, m.category, m.key, m.agent_id, m.updated_at, m.text
    FROM memories m LEFT JOIN projects p ON p.id = m.project_id";

fn memory_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ExportMemory> {
    Ok(ExportMemory {
        id: memory_ref(r.get(0)?),
        scope: r.get(1)?,
        project: r.get(2)?,
        task: r.get::<_, Option<i64>>(3)?.map(task_ref),
        category: r.get(4)?,
        key: r.get(5)?,
        agent: r.get(6)?,
        updated: iso8601(r.get(7)?),
        text: r.get(8)?,
    })
}

impl Hub {
    /// Lists Memories newest first, optionally narrowed by Scope and a Project
    /// ident substring.
    pub fn list_memories(
        &self,
        scope: Option<Scope>,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ExportMemory>> {
        let mut sql = format!("{MEMORY_COLUMNS} WHERE 1 = 1");
        let mut args = Vec::new();
        if let Some(scope) = scope {
            sql.push_str(" AND m.scope = ?");
            args.push(Value::Text(scope.as_str().into()));
        }
        if let Some(project) = project {
            sql.push_str(" AND instr(p.ident, ?) > 0");
            args.push(Value::Text(project.into()));
        }
        sql.push_str(" ORDER BY m.updated_at DESC, m.id DESC LIMIT ?");
        args.push(Value::Integer(limit as i64));
        self.query_rows(&sql, params_from_iter(args), memory_from_row)
    }

    pub fn show_memory(&self, id: &str) -> Result<MemoryDetail> {
        let id = parse_ref(id, 'm', "memory")?;
        let memory = self
            .conn
            .query_row(
                &format!("{MEMORY_COLUMNS} WHERE m.id = ?1"),
                params![id],
                memory_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::rejected(format!("{} not found", memory_ref(id))))?;
        let revisions = self.query_rows(
            "SELECT agent_id, created_at, category, text FROM revisions WHERE memory_id = ?1 ORDER BY id DESC",
            params![id],
            |r| {
                Ok(ExportRevision {
                    agent: r.get(0)?,
                    written: iso8601(r.get(1)?),
                    category: r.get(2)?,
                    text: r.get(3)?,
                })
            },
        )?;
        Ok(MemoryDetail { memory, revisions })
    }

    pub fn export(&self) -> Result<Export> {
        let memories = self.query_rows(
            &format!("{MEMORY_COLUMNS} ORDER BY m.scope, p.ident, m.task_id, m.id"),
            params![],
            memory_from_row,
        )?;
        let tasks = self.query_rows(
            "SELECT t.id, p.ident, t.title, t.status, t.outcome, t.updated_at, t.brief
             FROM tasks t JOIN projects p ON p.id = t.project_id ORDER BY p.ident, t.id",
            params![],
            |r| {
                Ok(ExportTask {
                    id: task_ref(r.get(0)?),
                    project: r.get(1)?,
                    title: r.get(2)?,
                    status: r.get(3)?,
                    outcome: r.get(4)?,
                    updated: iso8601(r.get(5)?),
                    brief: r.get(6)?,
                })
            },
        )?;
        Ok(Export { memories, tasks })
    }

    /// Deletes any Memory by id, regardless of Scope.
    pub fn delete_memory(&self, id: &str) -> Result<()> {
        let id = parse_ref(id, 'm', "memory")?;
        if self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])?
            == 0
        {
            return Err(Error::rejected(format!("{} not found", memory_ref(id))));
        }
        Ok(())
    }
}
