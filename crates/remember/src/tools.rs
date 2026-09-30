//! Tool definitions and dispatch. Schemas are hand-written and minimal because
//! every byte here sits in each Agent's context for the whole conversation.

use std::sync::Arc;

use remember_core::{Category, Error, Hub, NewMemory, RecallQuery, Scope, toon};
use rmcp::model::{JsonObject, Tool};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub const INSTRUCTIONS: &str = "\
Remember is shared memory for coding agents (Claude Code, Codex, opencode...), so work survives switching agents.
- Call `context` first in every conversation.
- If the user wants to continue earlier work, find it with `task` list, `join` it, and read its brief before acting.
- For multi-step work, `task` start one and keep its `brief` current after each meaningful step: another agent may take over if you stop.
- Save durable knowledge with `remember`: decisions, facts, gotchas. One idea per memory; give a key to anything that can change.
- Scopes: user = about the human, all projects; project = this repo; task = only the current task.
- `recall` before re-investigating something another agent may already know.
- Before closing a task, promote its memories that stay true.
- When a result ends with `inbox: N unread`, call `inbox`.
- Never store secrets.";

fn tool(name: &'static str, description: &'static str, schema: Value) -> Tool {
    let Value::Object(schema) = schema else {
        unreachable!("schemas are objects")
    };
    Tool::new(name, description, Arc::new(schema))
}

fn object(properties: Value, required: &[&str]) -> Value {
    if required.is_empty() {
        json!({"type": "object", "properties": properties})
    } else {
        json!({"type": "object", "properties": properties, "required": required})
    }
}

pub fn definitions() -> Vec<Tool> {
    let scope = json!({"enum": ["user", "project", "task"]});
    let category = json!({"enum": ["fact", "decision", "todo", "note"]});
    vec![
        tool(
            "context",
            "Call first in every conversation. Returns your agent id, user memories, project decisions and open tasks.",
            object(
                json!({"label": {"type": "string", "description": "short role name, e.g. backend"}}),
                &[],
            ),
        ),
        tool(
            "remember",
            "Save one memory (max 1000 chars). A key makes it replaceable: same key and scope overwrites.",
            object(
                json!({
                    "text": {"type": "string"},
                    "scope": scope,
                    "category": category,
                    "key": {"type": "string", "description": "e.g. auth.mechanism"}
                }),
                &["text", "scope", "category"],
            ),
        ),
        tool(
            "recall",
            "Search memories visible to you (current task, project, user), best match first. No query: most recent.",
            object(
                json!({
                    "query": {"type": "string"},
                    "scope": scope,
                    "category": category,
                    "key": {"type": "string"},
                    "include_done": {"type": "boolean", "description": "also search done tasks"},
                    "limit": {"type": "integer"}
                }),
                &[],
            ),
        ),
        tool(
            "forget",
            "Delete a memory by id.",
            object(json!({"id": {"type": "string"}}), &["id"]),
        ),
        tool(
            "task",
            "Tasks are shared units of work any agent can join. list; start {title, brief?} creates and joins; \
             join {id} returns brief and memories; leave; close {outcome, promote?: memory ids to keep in project, id?}; reopen {id}.",
            object(
                json!({
                    "action": {"enum": ["list", "start", "join", "leave", "close", "reopen"]},
                    "id": {"type": "string"},
                    "title": {"type": "string"},
                    "brief": {"type": "string"},
                    "outcome": {"type": "string"},
                    "promote": {"type": "array", "items": {"type": "string"}},
                    "include_done": {"type": "boolean"}
                }),
                &["action"],
            ),
        ),
        tool(
            "brief",
            "Read (no text) or replace the current task's brief: goal, state, next steps, open questions. \
             Update after each meaningful step so another agent can take over. Writes need base_revision.",
            object(
                json!({"text": {"type": "string"}, "base_revision": {"type": "integer"}}),
                &[],
            ),
        ),
        tool(
            "send",
            "Message an online agent (codex#91bc) or a task (t7: every agent in or later joining it gets it).",
            object(
                json!({"to": {"type": "string"}, "text": {"type": "string"}}),
                &["to", "text"],
            ),
        ),
        tool(
            "inbox",
            "Read your unread messages.",
            object(json!({}), &[]),
        ),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextArgs {
    label: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RememberArgs {
    text: String,
    scope: String,
    category: String,
    key: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecallArgs {
    query: Option<String>,
    scope: Option<String>,
    category: Option<String>,
    key: Option<String>,
    #[serde(default)]
    include_done: bool,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskArgs {
    action: String,
    id: Option<String>,
    title: Option<String>,
    brief: Option<String>,
    outcome: Option<String>,
    #[serde(default)]
    promote: Vec<String>,
    #[serde(default)]
    include_done: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BriefArgs {
    text: Option<String>,
    base_revision: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    to: String,
    text: String,
}

fn parse<T: DeserializeOwned>(args: Option<JsonObject>) -> Result<T, Error> {
    serde_json::from_value(Value::Object(args.unwrap_or_default()))
        .map_err(|e| Error::rejected(format!("invalid arguments: {e}")))
}

fn need<'a>(value: &'a Option<String>, field: &str, action: &str) -> Result<&'a str, Error> {
    value
        .as_deref()
        .ok_or_else(|| Error::rejected(format!("task {action} needs {field}")))
}

/// Runs one tool for `agent` and returns the TOON (or bare) result text.
pub fn call(
    hub: &mut Hub,
    agent: &str,
    name: &str,
    args: Option<JsonObject>,
) -> Result<String, Error> {
    match name {
        "context" => {
            let args: ContextArgs = parse(args)?;
            toon::encode(&hub.context(agent, args.label.as_deref())?)
        }
        "remember" => {
            let args: RememberArgs = parse(args)?;
            hub.remember(
                agent,
                NewMemory {
                    text: &args.text,
                    scope: Scope::parse(&args.scope)?,
                    category: Category::parse(&args.category)?,
                    key: args.key.as_deref(),
                },
            )
        }
        "recall" => {
            let args: RecallArgs = parse(args)?;
            let query = RecallQuery {
                text: args.query.as_deref(),
                scope: args.scope.as_deref().map(Scope::parse).transpose()?,
                category: args.category.as_deref().map(Category::parse).transpose()?,
                key: args.key.as_deref(),
                include_done: args.include_done,
                limit: args.limit,
            };
            let recall = hub.recall(agent, &query)?;
            if recall.memories.is_empty() && recall.truncated.is_none() {
                return Ok("no memories found".to_string());
            }
            toon::encode(&recall)
        }
        "forget" => {
            let args: IdArgs = parse(args)?;
            hub.forget(agent, &args.id)
        }
        "task" => {
            let args: TaskArgs = parse(args)?;
            match args.action.as_str() {
                "list" => {
                    let list = hub.list_tasks(agent, args.include_done)?;
                    if list.tasks.is_empty() {
                        return Ok("no tasks; start one with task action=start".to_string());
                    }
                    toon::encode(&list)
                }
                "start" => {
                    let title = need(&args.title, "title", "start")?;
                    toon::encode(&hub.start_task(agent, title, args.brief.as_deref())?)
                }
                "join" => toon::encode(&hub.join_task(agent, need(&args.id, "id", "join")?)?),
                "leave" => hub.leave_task(agent),
                "close" => {
                    let outcome = need(&args.outcome, "outcome", "close")?;
                    hub.close_task(agent, args.id.as_deref(), outcome, &args.promote)
                }
                "reopen" => toon::encode(&hub.reopen_task(agent, need(&args.id, "id", "reopen")?)?),
                other => Err(Error::rejected(format!(
                    "unknown task action '{other}'; use list, start, join, leave, close or reopen"
                ))),
            }
        }
        "brief" => {
            let args: BriefArgs = parse(args)?;
            toon::encode(&hub.brief(agent, args.text.as_deref(), args.base_revision)?)
        }
        "send" => {
            let args: SendArgs = parse(args)?;
            hub.send(agent, &args.to, &args.text)
        }
        "inbox" => {
            let inbox = hub.inbox(agent)?;
            if inbox.messages.is_empty() {
                return Ok("no unread messages".to_string());
            }
            toon::encode(&inbox)
        }
        other => Err(Error::rejected(format!("unknown tool '{other}'"))),
    }
}
