//! Agent-facing payloads. Field order here is the column order Agents see in TOON.

use serde::Serialize;

use crate::model::estimate_tokens;

#[derive(Debug, Serialize)]
pub struct MemoryRow {
    pub id: String,
    pub scope: String,
    pub category: String,
    pub key: Option<String>,
    pub age: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct Recall {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub memories: Vec<MemoryRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserRow {
    pub id: String,
    pub key: Option<String>,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct DecisionRow {
    pub id: String,
    pub key: Option<String>,
    pub age: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub status: String,
    pub last_agent: String,
    pub age: String,
}

/// An online Agent in the same Project and what it is working on.
#[derive(Debug, Serialize)]
pub struct AgentRow {
    pub id: String,
    pub label: Option<String>,
    pub task: Option<String>,
    pub seen: String,
}

#[derive(Debug, Serialize)]
pub struct Context {
    pub agent: String,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub user: Vec<UserRow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<DecisionRow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AgentRow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TaskRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<String>,
}

/// A Task in `task list`, with the online Agents currently in it.
#[derive(Debug, Serialize)]
pub struct TaskListRow {
    pub id: String,
    pub title: String,
    pub status: String,
    pub agents: String,
    pub last_agent: String,
    pub age: String,
}

#[derive(Debug, Serialize)]
pub struct TaskList {
    pub tasks: Vec<TaskListRow>,
}

#[derive(Debug, Serialize)]
pub struct TaskView {
    pub task: String,
    pub title: String,
    pub status: String,
    pub brief_revision: i64,
    pub brief: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub memories: Vec<MemoryRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Brief {
    pub task: String,
    pub brief_revision: i64,
    pub brief: String,
}

#[derive(Debug, Serialize)]
pub struct MessageRow {
    pub from: String,
    pub to: String,
    pub age: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct Inbox {
    pub messages: Vec<MessageRow>,
}

/// Approximate tokens one row adds to a TOON table: its fields, one delimiter
/// or quote allowance per field, and the line break.
pub fn row_tokens(fields: &[&str]) -> usize {
    fields.iter().map(|f| estimate_tokens(f) + 1).sum::<usize>() + 1
}

/// Keeps the longest prefix of `rows` whose estimated cost fits in `budget`.
/// Returns how many rows were dropped.
pub fn fit<T>(rows: &mut Vec<T>, budget: &mut usize, cost: impl Fn(&T) -> usize) -> usize {
    let mut keep = 0;
    for row in rows.iter() {
        let c = cost(row);
        if c > *budget {
            break;
        }
        *budget -= c;
        keep += 1;
    }
    let dropped = rows.len() - keep;
    rows.truncate(keep);
    dropped
}
