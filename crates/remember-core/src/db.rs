//! Opening the shared SQLite file and migrating its schema (see ADR 0001).

use std::fs::{self, OpenOptions};
use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};

use crate::error::{Error, Result};

/// Forward-only migrations; entry `i` moves the schema from version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE projects (
    id INTEGER PRIMARY KEY,
    ident TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    label TEXT,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    current_task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL,
    started_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    ended_at INTEGER
);

CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    title TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'done')),
    outcome TEXT,
    brief TEXT NOT NULL DEFAULT '',
    brief_revision INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL,
    last_agent_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    closed_at INTEGER
);
CREATE INDEX tasks_by_project ON tasks (project_id, status, updated_at);

CREATE TABLE memories (
    id INTEGER PRIMARY KEY,
    scope TEXT NOT NULL CHECK (scope IN ('user', 'project', 'task')),
    project_id INTEGER REFERENCES projects(id),
    task_id INTEGER REFERENCES tasks(id),
    category TEXT NOT NULL CHECK (category IN ('fact', 'decision', 'todo', 'note')),
    key TEXT,
    text TEXT NOT NULL,
    -- FNV-1a of category and text: finds exact duplicates without indexing the text itself.
    text_hash INTEGER NOT NULL,
    agent_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK ((scope = 'user' AND project_id IS NULL AND task_id IS NULL)
        OR (scope = 'project' AND project_id IS NOT NULL AND task_id IS NULL)
        OR (scope = 'task' AND project_id IS NOT NULL AND task_id IS NOT NULL))
);
CREATE UNIQUE INDEX memories_by_key
    ON memories (scope, ifnull(project_id, 0), ifnull(task_id, 0), key)
    WHERE key IS NOT NULL;
CREATE INDEX memories_by_bucket ON memories (scope, project_id, task_id, updated_at);
CREATE INDEX memories_by_hash ON memories (text_hash);
CREATE INDEX memories_by_recency ON memories (updated_at);

CREATE TABLE revisions (
    id INTEGER PRIMARY KEY,
    memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    category TEXT NOT NULL,
    text TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX revisions_by_memory ON revisions (memory_id, id);

-- External-content index: text is stored once, in `memories`.
CREATE VIRTUAL TABLE memories_fts USING fts5(
    key, text,
    content = 'memories', content_rowid = 'id',
    tokenize = 'porter unicode61'
);
CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts (rowid, key, text) VALUES (new.id, new.key, new.text);
END;
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts (memories_fts, rowid, key, text) VALUES ('delete', old.id, old.key, old.text);
END;
CREATE TRIGGER memories_au AFTER UPDATE OF key, text ON memories BEGIN
    INSERT INTO memories_fts (memories_fts, rowid, key, text) VALUES ('delete', old.id, old.key, old.text);
    INSERT INTO memories_fts (rowid, key, text) VALUES (new.id, new.key, new.text);
END;

CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    from_agent_id TEXT NOT NULL,
    to_agent_id TEXT,
    to_task_id INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
    text TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    CHECK ((to_agent_id IS NULL) <> (to_task_id IS NULL))
);
CREATE INDEX messages_to_agent ON messages (to_agent_id) WHERE to_agent_id IS NOT NULL;
CREATE INDEX messages_to_task ON messages (to_task_id) WHERE to_task_id IS NOT NULL;

CREATE TABLE message_reads (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL,
    read_at INTEGER NOT NULL,
    PRIMARY KEY (message_id, agent_id)
) WITHOUT ROWID;
"#];

pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// Opens (creating if needed) the DB at `path`, owner-only, and migrates it.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    create_private(path)?;
    let mut conn = Connection::open(path)?;
    configure(&mut conn)?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    configure(&mut conn)?;
    migrate(&mut conn)?;
    Ok(conn)
}

#[cfg(unix)]
fn create_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> Result<()> {
    OpenOptions::new().create(true).append(true).open(path)?;
    Ok(())
}

fn configure(conn: &mut Connection) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // Every transaction here reads then writes. A deferred one would fail with
    // SQLITE_BUSY when upgrading to a write lock, bypassing the busy timeout;
    // taking the write lock up front makes concurrent Agents queue instead.
    conn.set_transaction_behavior(TransactionBehavior::Immediate);
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Reads go through the OS page cache via mmap instead of a private copy:
    // recall touches scattered rows, and the default 2MB cache thrashes long
    // before 50k Memories while mapped pages cost no private memory.
    conn.pragma_update(None, "mmap_size", 256 * 1024 * 1024)?;
    Ok(())
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Fails when another binary has already moved the schema past this one.
pub fn ensure_supported(conn: &Connection) -> Result<()> {
    let found = schema_version(conn)?;
    if found > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(())
}

fn migrate(conn: &mut Connection) -> Result<()> {
    ensure_supported(conn)?;
    if schema_version(conn)? == SCHEMA_VERSION {
        return Ok(());
    }
    // Exclusive so two Agents starting at once cannot both apply a migration.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Exclusive)?;
    let current = schema_version(&tx)?;
    if current > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    for (version, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version as i64 + 1)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_fresh_file_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/remember.db");
        let conn = open(&path).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        drop(conn);
        // Reopening an up-to-date file is a no-op.
        open(&path).unwrap();
    }

    #[test]
    fn refuses_newer_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("remember.db");
        let conn = open(&path).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        let err = open(&path).unwrap_err();
        assert!(matches!(err, Error::SchemaTooNew { .. }));
        assert!(err.to_string().contains("restart the agent"));
    }
}
