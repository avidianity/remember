# Remember

Remember is a shared memory and messaging MCP server for coding agents.
Claude Code, Codex, opencode and any other MCP client can save what they learn, pick up each other's unfinished work, and leave messages for one another.
Switching agents mid-task stops meaning starting over.

Everything Remember returns to an agent is [TOON](https://github.com/toon-format/spec), a compact encoding of JSON built for LLM context windows.

## How it works

Every agent spawns its own `remember serve` process over stdio.
All of them share one SQLite file, so there is no daemon to run.
The server registers the agent automatically from the MCP handshake: its kind comes from the client name and its project from the working directory (the normalized git `origin` URL, else the git root, else the directory).

Knowledge lives in three scopes:

- **User**: facts about you and your preferences, visible in every project.
- **Project**: facts and decisions about one codebase, visible to every agent working in it.
- **Task**: notes for one unit of work, visible only to agents that joined that task.

A **task** is a unit of work that agents join, one after another or side by side.
Each task has a **brief** (goal, current state, next steps, open questions) that agents keep current as they work.
If an agent crashes or hits a rate limit, the next one joins the task, reads the brief, and continues.

A memory can carry a **key** (`auth.mechanism`).
Saving the same key in the same scope replaces the old value and keeps it as a revision, so agents never read stale, contradicting memories.

## Tools

| Tool | Purpose |
| --- | --- |
| `context` | Call first. Returns the agent's id, user memories, project decisions, the other agents online in the project and what they are working on, and open tasks. |
| `remember` | Save one memory to a scope, optionally keyed. |
| `recall` | Search visible memories, best match first. |
| `forget` | Delete a memory by id. |
| `task` | `list` (with the agents in each task), `start`, `join`, `leave`, `close` (promoting memories to the project) or `reopen` a task. |
| `brief` | Read or replace the current task's brief. Writes carry the revision they read, so concurrent edits are never lost. |
| `send` | Message an online agent (`codex#91bc`) or a task (`t7`). With `wake`, also nudge idle recipients. |
| `inbox` | Read unread messages, or with `wait` block until one arrives. Any tool result ends with `inbox: N unread` while mail is waiting. |

A first call looks like this:

```
agent: codex#2e59
project: github.com/acme/app
decisions[1]{id,key,age,text}:
  m1,ratelimit.store,4m,Rate limit counters live in Redis.
agents[1]{id,label,task,seen}:
  claude-code#3f2f,backend,t1,now
tasks[1]{id,title,status,last_agent,age}:
  t1,Add rate limiting,open,claude-code#3f2f,4m
```

## Talking live

Two agents working side by side converse by sending a message and then calling `inbox` with `wait` (seconds), which returns as soon as the reply lands.
The wait is capped at 50 seconds, under every supported client's tool-call timeout; call `inbox` again to keep waiting.

An agent sitting idle at its prompt only reacts to typed input.
If it runs inside tmux, `send` with `wake: true` types this line into its pane, which starts a turn:

```
remember: new message from claude-code#3f2f, call the inbox tool
```

Only that fixed line is ever typed, never the message itself, so no agent can inject keystrokes into another's terminal.
Codex hides the tmux variables from the servers it spawns; `remember setup codex` shows the `env_vars` line that forwards them.

## Install

### Quick install (Linux and macOS)

```sh
sh -c "$(curl -fsSL https://raw.githubusercontent.com/avidianity/remember/main/install.sh)"
```

This downloads the prebuilt binary for your platform, verifies its SHA-256 checksum, and installs it to `~/.local/bin`.
Override the location with `REMEMBER_INSTALL_DIR`, or pin a version with `REMEMBER_VERSION=v0.2.0`.

### Prebuilt binaries

Download the archive for your platform from [Releases](https://github.com/avidianity/remember/releases) (Linux, macOS and Windows, with SHA-256 checksums), unpack it, and put `remember` on your `PATH`.

### From source

```sh
cargo install --git https://github.com/avidianity/remember remember
```

### Register with your agents

Register Remember with each agent you use:

```sh
remember setup claude-code
remember setup codex
remember setup opencode
```

`setup` runs the agent's own `mcp add` command, after showing it and asking for confirmation.
It pins the absolute database path in the agent's config as `REMEMBER_DB`, because agents pass different environments to the servers they spawn and a path derived from the environment could split your memory across two files.

Codex asks for approval before every MCP call, and hides your tmux pane from the servers it spawns.
To let Remember's tools run without prompts and let other agents wake Codex, add this to `~/.codex/config.toml`:

```toml
[mcp_servers.remember]
default_tools_approval_mode = "approve"
env_vars = ["TMUX", "TMUX_PANE"]
```

## Browsing memory yourself

```sh
remember ls --scope project --project acme/app
remember show m42
remember export --format md
remember forget m42
```

Output is TOON by default; pass `--format md` for Markdown.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `REMEMBER_DB` | `$HOME/.local/share/remember/remember.db` | Absolute path to the shared database. |
| `REMEMBER_MEMORY_CHARS` | `1000` | Longest memory accepted. |
| `REMEMBER_BRIEF_CHARS` | `4000` | Longest brief accepted. |
| `REMEMBER_CONTEXT_TOKENS` | `1500` | Token budget of the `context` payload. |
| `REMEMBER_RECALL_TOKENS` | `800` | Token budget of a `recall` result. |
| `REMEMBER_JOIN_TOKENS` | `1500` | Token budget of a `task join` result. |

The database file is created readable only by its owner.
Remember rejects text that looks like a credential (private keys, cloud and API tokens, JWTs, high-entropy `KEY=value` pairs); store a reference to the secret instead.

## Efficiency budgets

These are enforced by tests in CI:

- All tool definitions together fit in 1,200 tokens, and the server instructions in 1,500 characters.
- `context` stays within 1,500 tokens and `recall` within 800, measured with a real tokenizer.
- Every operation has a p99 latency under 10 ms on a database with 50,000 memories.
- The server is ready in about 2 ms and idles at under 1 MB of private memory.

## Development

```sh
cargo test --workspace
cargo test --release -p remember-core --test perf -- --ignored --nocapture
```

The domain language is defined in [CONTEXT.md](CONTEXT.md), and the decisions behind the design are recorded in [docs/adr](docs/adr).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
