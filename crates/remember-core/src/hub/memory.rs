use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Transaction, params, params_from_iter};

use super::{Caller, Hub, check_len, check_secret, required};
use crate::error::{Error, Result};
use crate::model::{Category, Scope, age, memory_ref, parse_ref, task_ref};
use crate::views::{MemoryRow, Recall, fit, row_tokens};

pub struct NewMemory<'a> {
    pub text: &'a str,
    pub scope: Scope,
    pub category: Category,
    pub key: Option<&'a str>,
}

/// Which Scope bucket a Memory lives in: (project_id, task_id).
type Bucket = (Option<i64>, Option<i64>);

impl Hub {
    /// Saves a Memory. Returns `m42` when new, `m42 updated` when a keyed Memory
    /// was replaced, or `m42 exists` when nothing changed.
    pub fn remember(&mut self, agent_id: &str, memory: NewMemory<'_>) -> Result<String> {
        let caller = self.caller(agent_id)?;
        let text = required("memory text", memory.text)?;
        check_len("memory", text, self.limits.memory_chars)?;
        let key = memory.key.map(str::trim).filter(|k| !k.is_empty());
        if let Some(key) = key {
            validate_key(key)?;
        }
        check_secret(text)?;
        let bucket = self.bucket(&caller, memory.scope)?;
        let category = memory.category.as_str();
        let now = self.now();
        let keep = self.limits.revisions_kept;

        let tx = self.conn.transaction()?;
        let result = match key {
            Some(key) => upsert_keyed(
                &tx,
                memory.scope,
                bucket,
                key,
                category,
                text,
                agent_id,
                now,
                keep,
            )?,
            None => insert_unkeyed(&tx, memory.scope, bucket, category, text, agent_id, now)?,
        };
        if let (_, Some(task_id)) = bucket {
            touch_task(&tx, task_id, agent_id, now)?;
        }
        tx.commit()?;
        Ok(result)
    }

    fn bucket(&self, caller: &Caller, scope: Scope) -> Result<Bucket> {
        match scope {
            Scope::User => Ok((None, None)),
            Scope::Project => Ok((Some(caller.project_id), None)),
            Scope::Task => {
                let task_id = caller
                    .current_task_id
                    .ok_or_else(|| Error::rejected("no current task; join or start one"))?;
                Ok((Some(caller.project_id), Some(task_id)))
            }
        }
    }

    /// Searches every Memory visible to the Agent: its Current Task, its
    /// Project and the User Scope, ranked by relevance then recency.
    ///
    /// BM25 ranking costs about a microsecond per matching Memory, and a term
    /// found in a large share of Memories carries almost no ranking signal. So
    /// relevance is computed over informative terms only: first requiring all
    /// of them, then any of them when that finds too few. When every term is
    /// common, matches are listed newest first by walking the recency index,
    /// which stops as soon as enough visible matches are found.
    pub fn recall(&self, agent_id: &str, query: &RecallQuery<'_>) -> Result<Recall> {
        let caller = self.caller(agent_id)?;
        let limit = query
            .limit
            .unwrap_or(self.limits.recall_default)
            .clamp(1, self.limits.recall_max);
        let terms = query.text.map(search_terms).unwrap_or_default();
        let mut rows = if terms.is_empty() {
            self.recall_rows(&caller, query, None, limit, Order::Recent)?
        } else {
            let informative = self.informative_terms(&terms)?;
            let (terms, order) = if informative.is_empty() {
                (terms, Order::Recent)
            } else {
                (informative, Order::Relevance)
            };
            let all = self.recall_rows(&caller, query, Some(&terms.join(" AND ")), limit, order)?;
            if all.len() > limit || terms.len() == 1 {
                all
            } else {
                self.recall_rows(&caller, query, Some(&terms.join(" OR ")), limit, order)?
            }
        };
        let more = rows.len() > limit;
        rows.truncate(limit);
        let mut budget = self.limits.recall_tokens;
        let dropped = fit(&mut rows, &mut budget, memory_row_tokens);
        let truncated = match (dropped, more) {
            (0, false) => None,
            (0, true) => Some("more matches; narrow the query or raise limit".to_string()),
            (n, _) => Some(format!("{n} more over token budget; narrow the query")),
        };
        Ok(Recall {
            memories: rows,
            truncated,
        })
    }

    /// Terms matching at most 5% of all Memories (never fewer than 500).
    fn informative_terms(&self, terms: &[String]) -> Result<Vec<String>> {
        let total: i64 =
            self.conn
                .query_row("SELECT ifnull(max(id), 0) FROM memories", [], |r| r.get(0))?;
        let common = (total / 20).max(500);
        let mut informative = Vec::new();
        for term in terms {
            let matches: i64 = self.conn.query_row(
                "SELECT count(*) FROM memories_fts WHERE memories_fts MATCH ?1",
                params![term],
                |r| r.get(0),
            )?;
            if matches <= common {
                informative.push(term.clone());
            }
        }
        Ok(informative)
    }

    /// Up to `limit + 1` visible rows, so callers can tell whether more exist.
    fn recall_rows(
        &self,
        caller: &Caller,
        query: &RecallQuery<'_>,
        fts: Option<&str>,
        limit: usize,
        order: Order,
    ) -> Result<Vec<MemoryRow>> {
        let now = self.now();
        let mut args: Vec<Value> = Vec::new();
        let mut sql = String::from(
            "SELECT m.id, m.scope, m.task_id, m.category, m.key, m.updated_at, m.text FROM memories m",
        );
        match (order, fts) {
            (Order::Relevance, Some(fts)) => {
                sql.push_str(
                    " JOIN memories_fts f ON f.rowid = m.id WHERE memories_fts MATCH ?1 AND ",
                );
                args.push(Value::Text(fts.into()));
            }
            (_, Some(fts)) => {
                sql.push_str(
                    " INDEXED BY memories_by_recency WHERE m.id IN \
                     (SELECT rowid FROM memories_fts WHERE memories_fts MATCH ?1) AND ",
                );
                args.push(Value::Text(fts.into()));
            }
            (_, None) => sql.push_str(" INDEXED BY memories_by_recency WHERE "),
        }
        let (visible, visible_args) = visibility(caller, query.scope, query.include_done);
        sql.push_str(&visible);
        args.extend(visible_args);
        if let Some(category) = query.category {
            sql.push_str(" AND m.category = ?");
            args.push(Value::Text(category.as_str().into()));
        }
        if let Some(key) = query.key {
            sql.push_str(" AND m.key = ?");
            args.push(Value::Text(key.trim().into()));
        }
        sql.push_str(match (order, fts) {
            (Order::Relevance, Some(_)) => {
                " ORDER BY bm25(memories_fts, 2.0, 1.0), m.updated_at DESC, m.id DESC LIMIT ?"
            }
            _ => " ORDER BY m.updated_at DESC, m.id DESC LIMIT ?",
        });
        args.push(Value::Integer(limit as i64 + 1));
        self.query_rows(&sql, params_from_iter(args), |r| {
            let scope: String = r.get(1)?;
            let task_id: Option<i64> = r.get(2)?;
            Ok(MemoryRow {
                id: memory_ref(r.get(0)?),
                scope: task_id.map(task_ref).unwrap_or(scope),
                category: r.get(3)?,
                key: r.get(4)?,
                age: age(now - r.get::<_, i64>(5)?),
                text: r.get(6)?,
            })
        })
    }

    /// Deletes a visible Memory and its Revisions.
    pub fn forget(&self, agent_id: &str, id: &str) -> Result<String> {
        let caller = self.caller(agent_id)?;
        let id = parse_ref(id, 'm', "memory")?;
        let (visible, mut args) = visibility(&caller, None, true);
        args.insert(0, Value::Integer(id));
        let sql = format!(
            "DELETE FROM memories WHERE id = ?1 AND id IN (SELECT m.id FROM memories m WHERE {visible})"
        );
        let deleted = self.conn.execute(&sql, params_from_iter(args))?;
        if deleted == 0 {
            return Err(Error::rejected(format!("{} not found", memory_ref(id))));
        }
        Ok("ok".to_string())
    }
}

#[derive(Clone, Copy)]
enum Order {
    Relevance,
    Recent,
}

#[derive(Default)]
pub struct RecallQuery<'a> {
    pub text: Option<&'a str>,
    pub scope: Option<Scope>,
    pub category: Option<Category>,
    pub key: Option<&'a str>,
    pub include_done: bool,
    pub limit: Option<usize>,
}

pub(super) fn memory_row_tokens(m: &MemoryRow) -> usize {
    row_tokens(&[
        &m.id,
        &m.scope,
        &m.category,
        m.key.as_deref().unwrap_or("null"),
        &m.age,
        &m.text,
    ])
}

/// SQL condition (over alias `m`) selecting Memories the caller may see.
/// Placeholders are positional `?` so callers can prepend their own.
fn visibility(caller: &Caller, scope: Option<Scope>, include_done: bool) -> (String, Vec<Value>) {
    let project = Value::Integer(caller.project_id);
    let current = caller
        .current_task_id
        .map(Value::Integer)
        .unwrap_or(Value::Null);
    let task_clause = if include_done {
        "(m.scope = 'task' AND (m.task_id = ? OR m.task_id IN (SELECT id FROM tasks WHERE project_id = ? AND status = 'done')))"
    } else {
        "(m.scope = 'task' AND m.task_id = ?)"
    };
    let task_args = if include_done {
        vec![current, project.clone()]
    } else {
        vec![current]
    };
    match scope {
        Some(Scope::User) => ("(m.scope = 'user')".into(), vec![]),
        Some(Scope::Project) => (
            "(m.scope = 'project' AND m.project_id = ?)".into(),
            vec![project],
        ),
        Some(Scope::Task) => (task_clause.into(), task_args),
        None => {
            let mut args = vec![project];
            args.extend(task_args);
            (
                format!(
                    "(m.scope = 'user' OR (m.scope = 'project' AND m.project_id = ?) OR {task_clause})"
                ),
                args,
            )
        }
    }
}

/// Words that match nearly every Memory, so they add ranking cost but no signal.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "do", "does", "for", "from", "how", "i", "in",
    "is", "it", "of", "on", "or", "that", "the", "this", "to", "was", "we", "what", "when",
    "where", "which", "who", "why", "with",
];

/// Turns free text into quoted FTS5 terms (the porter tokenizer handles word
/// forms). Any FTS5 syntax in the input is neutralized by the quoting.
fn search_terms(text: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        let word = word.to_lowercase();
        let term = format!("\"{word}\"");
        if !STOPWORDS.contains(&word.as_str()) && !terms.contains(&term) {
            terms.push(term);
        }
    }
    if terms.is_empty() {
        // A query of only stopwords still searches for them rather than nothing.
        terms = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| format!("\"{}\"", t.to_lowercase()))
            .collect();
    }
    terms
}

fn validate_key(key: &str) -> Result<()> {
    let ok = key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'));
    if !ok {
        return Err(Error::rejected(format!(
            "invalid key '{key}'; use up to 64 of a-z 0-9 . _ - /"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn upsert_keyed(
    tx: &Transaction<'_>,
    scope: Scope,
    (project_id, task_id): Bucket,
    key: &str,
    category: &str,
    text: &str,
    agent_id: &str,
    now: i64,
    keep: usize,
) -> Result<String> {
    let existing: Option<(i64, String, String)> = tx
        .query_row(
            "SELECT id, category, text FROM memories
             WHERE scope = ?1 AND ifnull(project_id, 0) = ifnull(?2, 0)
               AND ifnull(task_id, 0) = ifnull(?3, 0) AND key = ?4",
            params![scope.as_str(), project_id, task_id, key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match existing {
        Some((id, old_category, old_text)) if old_category == category && old_text == text => {
            Ok(format!("{} exists", memory_ref(id)))
        }
        Some((id, _, _)) => {
            replace(tx, id, category, text, agent_id, now, keep)?;
            Ok(format!("{} updated", memory_ref(id)))
        }
        None => {
            tx.execute(
                "INSERT INTO memories (scope, project_id, task_id, category, key, text, text_hash, agent_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![scope.as_str(), project_id, task_id, category, key, text, text_hash(category, text), agent_id, now],
            )?;
            Ok(memory_ref(tx.last_insert_rowid()))
        }
    }
}

/// Moves the current value of Memory `id` into its Revisions and writes the new one.
pub(super) fn replace(
    tx: &Transaction<'_>,
    id: i64,
    category: &str,
    text: &str,
    agent_id: &str,
    now: i64,
    keep: usize,
) -> Result<()> {
    tx.execute(
        "INSERT INTO revisions (memory_id, category, text, agent_id, created_at)
         SELECT id, category, text, agent_id, updated_at FROM memories WHERE id = ?1",
        params![id],
    )?;
    tx.execute(
        "UPDATE memories SET category = ?2, text = ?3, text_hash = ?4, agent_id = ?5, updated_at = ?6 WHERE id = ?1",
        params![id, category, text, text_hash(category, text), agent_id, now],
    )?;
    tx.execute(
        "DELETE FROM revisions WHERE memory_id = ?1 AND id NOT IN (
            SELECT id FROM revisions WHERE memory_id = ?1 ORDER BY id DESC LIMIT ?2)",
        params![id, keep as i64],
    )?;
    Ok(())
}

fn insert_unkeyed(
    tx: &Transaction<'_>,
    scope: Scope,
    (project_id, task_id): Bucket,
    category: &str,
    text: &str,
    agent_id: &str,
    now: i64,
) -> Result<String> {
    let hash = text_hash(category, text);
    let duplicate: Option<i64> = tx
        .query_row(
            "SELECT id FROM memories
             WHERE text_hash = ?6 AND scope = ?1 AND ifnull(project_id, 0) = ifnull(?2, 0)
               AND ifnull(task_id, 0) = ifnull(?3, 0) AND category = ?4 AND text = ?5",
            params![scope.as_str(), project_id, task_id, category, text, hash],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = duplicate {
        return Ok(format!("{} exists", memory_ref(id)));
    }
    tx.execute(
        "INSERT INTO memories (scope, project_id, task_id, category, text, text_hash, agent_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![scope.as_str(), project_id, task_id, category, text, hash, agent_id, now],
    )?;
    Ok(memory_ref(tx.last_insert_rowid()))
}

/// Stable 64-bit FNV-1a; persisted, so it must never change between versions.
fn text_hash(category: &str, text: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in category.bytes().chain([0]).chain(text.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash as i64
}

pub(super) fn touch_task(
    tx: &Transaction<'_>,
    task_id: i64,
    agent_id: &str,
    now: i64,
) -> Result<()> {
    tx.execute(
        "UPDATE tasks SET updated_at = ?3, last_agent_id = ?2 WHERE id = ?1",
        params![task_id, agent_id, now],
    )?;
    Ok(())
}
