//! End-to-end: spawn the real binary over stdio, exactly as an agent does, and
//! drive it with an MCP client announcing itself as each supported agent.

use std::path::{Path, PathBuf};

use remember_core::{Hub, Limits};
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

type Client = RunningService<RoleClient, ClientConfig>;

struct World {
    _tmp: tempfile::TempDir,
    project: PathBuf,
    db: PathBuf,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("app");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::write(
            project.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:acme/app.git\n",
        )
        .unwrap();
        let db = tmp.path().join("data/remember.db");
        World {
            _tmp: tmp,
            project,
            db,
        }
    }

    async fn spawn(&self, client_name: &str) -> Client {
        self.spawn_in(client_name, &self.project).await
    }

    async fn spawn_in(&self, client_name: &str, cwd: &Path) -> Client {
        self.spawn_with(client_name, cwd, &[]).await
    }

    async fn spawn_with(&self, client_name: &str, cwd: &Path, env: &[(&str, &str)]) -> Client {
        let db = self.db.clone();
        let cwd = cwd.to_path_buf();
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let transport = TokioChildProcess::new(
            tokio::process::Command::new(env!("CARGO_BIN_EXE_remember")).configure(move |cmd| {
                // Tests may run inside tmux; never let an Agent register the
                // developer's real pane.
                cmd.arg("serve")
                    .current_dir(&cwd)
                    .env("REMEMBER_DB", &db)
                    .env_remove("HOME")
                    .env_remove("TMUX")
                    .env_remove("TMUX_PANE")
                    .envs(env);
            }),
        )
        .unwrap();
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new(client_name, "1.0"),
        )
        .serve(transport)
        .await
        .unwrap()
    }

    fn hub(&self) -> Hub {
        Hub::open(&self.db, Limits::default()).unwrap()
    }
}

/// Calls a tool and returns (text, is_error), asserting the ADR 0003 contract:
/// exactly one text block and never `structuredContent`.
async fn call(client: &Client, tool: &str, args: Value) -> (String, bool) {
    let Value::Object(args) = args else {
        panic!("args must be an object")
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
        .await
        .unwrap();
    assert!(
        result.structured_content.is_none(),
        "{tool} returned structuredContent"
    );
    assert_eq!(
        result.content.len(),
        1,
        "{tool} returned {} blocks",
        result.content.len()
    );
    let text = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    (text, result.is_error.unwrap_or(false))
}

async fn ok(client: &Client, tool: &str, args: Value) -> String {
    let (text, is_error) = call(client, tool, args).await;
    assert!(!is_error, "{tool} failed: {text}");
    text
}

fn field<'a>(toon: &'a str, name: &str) -> &'a str {
    toon.lines()
        .find_map(|l| l.strip_prefix(&format!("{name}: ")))
        .unwrap_or_else(|| panic!("no {name} in:\n{toon}"))
}

#[tokio::test]
async fn crashed_agent_hands_off_to_another_kind() {
    let world = World::new();

    let claude = world.spawn("claude-code").await;
    let context = ok(&claude, "context", json!({"label": "backend"})).await;
    let claude_id = field(&context, "agent").to_string();
    assert!(claude_id.starts_with("claude-code#"), "{context}");
    assert_eq!(field(&context, "project"), "github.com/acme/app");

    ok(
        &claude,
        "remember",
        json!({"text": "Reply tersely", "scope": "user", "category": "fact", "key": "style"}),
    )
    .await;
    let started = ok(
        &claude,
        "task",
        json!({"action": "start", "title": "Refactor auth", "brief": "goal: auth in middleware"}),
    )
    .await;
    assert_eq!(field(&started, "task"), "t1");
    ok(&claude, "remember", json!({"text": "auth now lives in middleware/auth.rs", "scope": "task", "category": "decision", "key": "auth.location"})).await;
    ok(&claude, "brief", json!({"text": "goal: auth in middleware\nstate: middleware done\nnext: wire routes", "base_revision": 1})).await;
    ok(
        &claude,
        "send",
        json!({"to": "t1", "text": "routes in api/v2 are the tricky ones"}),
    )
    .await;

    // Simulate a rate-limit crash: the process dies without a clean shutdown.
    drop(claude);

    let codex = world.spawn("codex-mcp-client").await;
    let context = ok(&codex, "context", json!({})).await;
    assert!(field(&context, "agent").starts_with("codex#"), "{context}");
    assert!(
        context.contains("Reply tersely"),
        "user memories reach every agent kind:\n{context}"
    );
    assert!(
        context.contains(&format!("t1,Refactor auth,open,{claude_id}")),
        "{context}"
    );

    let joined = ok(&codex, "task", json!({"action": "join", "id": "t1"})).await;
    assert_eq!(field(&joined, "brief_revision"), "2");
    assert!(joined.contains("next: wire routes"), "{joined}");
    assert!(
        joined.contains("auth now lives in middleware/auth.rs"),
        "{joined}"
    );
    assert!(joined.ends_with("inbox: 1 unread"), "{joined}");

    let inbox = ok(&codex, "inbox", json!({})).await;
    assert!(
        inbox.contains("routes in api/v2 are the tricky ones"),
        "{inbox}"
    );
    let recall = ok(&codex, "recall", json!({"query": "middleware"})).await;
    assert!(
        !recall.contains("inbox:"),
        "inbox line only while mail is unread:\n{recall}"
    );

    let closed = ok(
        &codex,
        "task",
        json!({"action": "close", "outcome": "auth moved", "promote": ["m2"]}),
    )
    .await;
    assert_eq!(closed, "t1 done, promoted 1");

    let opencode = world.spawn("opencode").await;
    let recall = ok(&opencode, "recall", json!({"query": "auth"})).await;
    assert!(
        recall.contains("m2,project,decision,auth.location"),
        "{recall}"
    );
    codex.cancel().await.unwrap();
    opencode.cancel().await.unwrap();
}

#[tokio::test]
async fn errors_are_single_lines_flagged_as_errors() {
    let world = World::new();
    let client = world.spawn("claude-code").await;
    let (text, is_error) = call(
        &client,
        "remember",
        json!({"text": "x", "scope": "task", "category": "note"}),
    )
    .await;
    assert!(is_error);
    assert_eq!(text, "error: no current task; join or start one");
    let (text, is_error) = call(
        &client,
        "remember",
        json!({"text": "x", "scope": "user", "category": "note", "bogus": 1}),
    )
    .await;
    assert!(is_error);
    assert!(
        text.starts_with("error: invalid arguments: unknown field `bogus`"),
        "{text}"
    );
    let (text, _) = call(&client, "task", json!({"action": "join"})).await;
    assert_eq!(text, "error: task join needs id");
    assert_eq!(ok(&client, "recall", json!({})).await, "no memories found");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn clean_exit_marks_agent_offline() {
    let world = World::new();
    let a = world.spawn("claude-code").await;
    let b = world.spawn("codex-mcp-client").await;
    let b_id = field(&ok(&b, "context", json!({})).await, "agent").to_string();
    ok(&a, "send", json!({"to": b_id, "text": "hello"})).await;
    b.cancel().await.unwrap();
    let (text, is_error) = call(&a, "send", json!({"to": b_id, "text": "still there?"})).await;
    assert!(is_error);
    assert_eq!(text, "error: agent offline; address the task instead");
    a.cancel().await.unwrap();
}

#[tokio::test]
async fn projects_are_isolated_by_directory() {
    let world = World::new();
    let elsewhere = world.project.parent().unwrap().join("scratch");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let a = world.spawn("claude-code").await;
    let b = world.spawn_in("claude-code", &elsewhere).await;
    ok(
        &a,
        "remember",
        json!({"text": "app uses postgres", "scope": "project", "category": "fact"}),
    )
    .await;
    assert_eq!(
        ok(&b, "recall", json!({"query": "postgres"})).await,
        "no memories found"
    );
    a.cancel().await.unwrap();
    b.cancel().await.unwrap();
}

#[tokio::test]
async fn newer_schema_locks_out_running_server() {
    let world = World::new();
    let client = world.spawn("claude-code").await;
    ok(&client, "context", json!({})).await;
    let conn = rusqlite_bump(&world.db);
    let (text, is_error) = call(&client, "recall", json!({})).await;
    assert!(is_error);
    assert!(
        text.contains("is newer than this binary") && text.ends_with("restart the agent"),
        "{text}"
    );
    drop(conn);
    client.cancel().await.unwrap();
}

/// Simulates a newer binary migrating the shared DB.
fn rusqlite_bump(db: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.pragma_update(None, "user_version", remember_core::db::SCHEMA_VERSION + 1)
        .unwrap();
    conn
}

#[tokio::test]
async fn tool_surface_stays_within_budget() {
    let world = World::new();
    let client = world.spawn("claude-code").await;
    let info = client.peer_info().unwrap();
    let instructions = info.instructions.clone().unwrap();
    assert!(
        instructions.len() <= 1500,
        "instructions are {} chars",
        instructions.len()
    );
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(
        names,
        [
            "context", "remember", "recall", "forget", "task", "brief", "send", "inbox"
        ]
    );
    let schema = serde_json::to_string(&tools).unwrap();
    let tokens = tiktoken_rs::o200k_base_singleton()
        .encode_with_special_tokens(&schema)
        .len();
    assert!(tokens <= 1200, "tool definitions are {tokens} tokens");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn concurrent_agents_lose_no_writes() {
    let world = World::new();
    let mut clients = Vec::new();
    for n in 0..8 {
        clients.push(world.spawn(&format!("agent-{n}")).await);
    }
    let writers = clients.iter().enumerate().map(|(n, client)| async move {
        for i in 0..40 {
            ok(client, "remember", json!({"text": format!("agent {n} note {i}"), "scope": "project", "category": "note"})).await;
        }
    });
    futures_join(writers).await;
    let total = world.hub().list_memories(None, None, 10_000).unwrap().len();
    assert_eq!(total, 8 * 40);
    for client in clients {
        client.cancel().await.unwrap();
    }
}

async fn futures_join<F: std::future::Future<Output = ()>>(futures: impl Iterator<Item = F>) {
    let futures: Vec<_> = futures.map(Box::pin).collect();
    let mut pending = futures;
    // Poll all writers concurrently on the current-thread runtime.
    std::future::poll_fn(move |cx| {
        pending.retain_mut(|f| f.as_mut().poll(cx).is_pending());
        if pending.is_empty() {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
}

/// Real clients (Claude Code, Codex) stop servers with a signal instead of
/// closing stdin; the Agent must still be marked offline.
#[tokio::test]
async fn sigterm_marks_agent_offline() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let world = World::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_remember"))
        .arg("serve")
        .current_dir(&world.project)
        .env("REMEMBER_DB", &world.db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"claude-code","version":"1"}}}}}}"#
    )
    .unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("\"remember\""), "{line}");

    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    child.wait().unwrap();

    let conn = rusqlite::Connection::open(&world.db).unwrap();
    let ended: Option<i64> = conn
        .query_row(
            "SELECT ended_at FROM agents WHERE kind = 'claude-code'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ended.is_some(), "agent still looks online after SIGTERM");
}

#[tokio::test]
async fn inbox_wait_returns_as_soon_as_mail_arrives() {
    use std::time::{Duration, Instant};

    let world = World::new();
    let codex = world.spawn("codex-mcp-client").await;
    let claude = world.spawn("claude-code").await;
    let codex_id = field(&ok(&codex, "context", json!({})).await, "agent").to_string();

    let started = Instant::now();
    let idle = ok(&codex, "inbox", json!({"wait": 1})).await;
    let waited = started.elapsed();
    assert!(!idle.contains("claude-code#"), "{idle}");
    assert!(
        (Duration::from_millis(900)..Duration::from_secs(3)).contains(&waited),
        "empty wait took {waited:?}"
    );

    let sender = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        ok(
            &claude,
            "send",
            json!({"to": codex_id, "text": "schema is ready"}),
        )
        .await
    };
    let started = Instant::now();
    let (mail, sent) = tokio::join!(ok(&codex, "inbox", json!({"wait": 30})), sender);
    let waited = started.elapsed();
    assert_eq!(sent, "ok");
    assert!(mail.contains("schema is ready"), "{mail}");
    assert!(
        waited < Duration::from_secs(3),
        "wait ignored new mail for {waited:?}"
    );
}

/// A real tmux server stands in for the terminal an idle Agent sits in.
#[tokio::test]
async fn send_wake_types_a_nudge_into_the_recipients_tmux_pane() {
    use std::process::Command;
    use std::time::Duration;

    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("tmux not installed; skipping");
        return;
    }
    let world = World::new();
    let socket = world.db.with_file_name("tmux.sock");
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let socket = socket.to_str().unwrap().to_string();
    let tmux = |args: &[&str]| {
        let out = Command::new("tmux")
            .args(["-S", &socket, "-f", "/dev/null"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "tmux {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    // `cat` echoes whatever is typed, like an agent's input prompt.
    tmux(&["new-session", "-d", "-x", "200", "-y", "20", "cat"]);
    let pane = tmux(&["display-message", "-p", "#{pane_id}"])
        .trim()
        .to_string();
    let tmux_env = format!("{socket},0,0");

    let idle = world
        .spawn_with(
            "codex-mcp-client",
            &world.project,
            &[("TMUX", &tmux_env), ("TMUX_PANE", &pane)],
        )
        .await;
    let idle_id = field(&ok(&idle, "context", json!({})).await, "agent").to_string();
    let claude = world.spawn("claude-code").await;
    let claude_id = field(&ok(&claude, "context", json!({})).await, "agent").to_string();

    let sent = ok(
        &claude,
        "send",
        json!({"to": idle_id, "text": "rm -rf / ; review my PR", "wake": true}),
    )
    .await;
    let mut screen = String::new();
    for _ in 0..40 {
        screen = tmux(&["capture-pane", "-p", "-t", &pane]);
        if screen.contains("call the inbox tool") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = Command::new("tmux")
        .args(["-S", &socket, "kill-server"])
        .status();

    assert_eq!(sent, "ok, woke 1");
    assert!(
        screen.contains(&format!(
            "remember: new message from {claude_id}, call the inbox tool"
        )),
        "{screen}"
    );
    assert!(
        !screen.contains("rm -rf"),
        "message text must never be typed: {screen}"
    );
    let reply = ok(
        &idle,
        "send",
        json!({"to": claude_id, "text": "on it", "wake": true}),
    )
    .await;
    assert_eq!(
        reply.lines().next(),
        Some("ok, woke 0"),
        "sender outside tmux cannot be woken"
    );
}

/// Protocol 2026-07-28 has no `initialize`: the client names itself in each
/// request's `_meta`, and its SDK rejects list results without the cache
/// fields. Driven over raw JSON-RPC so the wire shape itself is checked.
#[tokio::test]
async fn handshake_less_client_lists_tools_and_registers() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let world = World::new();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_remember"))
        .arg("serve")
        .current_dir(&world.project)
        .env("REMEMBER_DB", &world.db)
        .env_remove("HOME")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "claude-code", "version": "1.0"},
    });
    let mut request = async |id: u64, method: &str, mut params: Value| -> Value {
        params["_meta"] = meta.clone();
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        let reply: Value =
            serde_json::from_str(&stdout.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(reply["id"], id, "{reply}");
        reply["result"].clone()
    };

    let tools = request(1, "tools/list", json!({})).await;
    assert!(tools["ttlMs"].is_u64(), "{tools}");
    assert!(
        matches!(tools["cacheScope"].as_str(), Some("public" | "private")),
        "{tools}"
    );
    assert_eq!(tools["tools"].as_array().unwrap().len(), 8);

    let context = request(2, "tools/call", json!({"name": "context", "arguments": {}})).await;
    assert_eq!(context["isError"], false, "{context}");
    let text = context["content"][0]["text"].as_str().unwrap();
    assert!(field(text, "agent").starts_with("claude-code#"), "{text}");
    let conn = rusqlite::Connection::open(&world.db).unwrap();
    let agents: i64 = conn
        .query_row("SELECT COUNT(*) FROM agents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(agents, 1, "one registration across both requests");
}
