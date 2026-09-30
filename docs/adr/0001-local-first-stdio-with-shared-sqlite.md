# Local-first: one stdio server per Agent, all sharing one SQLite file

Each Agent spawns its own Remember server over stdio, and all those processes read and write a single SQLite database in WAL mode under the user's data directory.
We chose this over a long-running HTTP daemon because the handoffs we care about happen on one developer machine, and a shared file needs no daemon to start, supervise, or secure.
The core stays transport-agnostic so a streamable HTTP transport can be added later for multi-machine use without reworking storage or tools.

## Consequences

- Concurrency is multi-process, not multi-threaded: every write must be a short transaction, and the server must tolerate `SQLITE_BUSY` with a busy timeout.
- One server process serves exactly one Agent, so Agent identity is held by the process after Registration and is never passed on tool calls.
- The server cannot push to an idle Agent; unread Messages are surfaced by a trailing line on every tool response, and live conversation relies on Wait and Wake (ADR 0004).
- Every Agent must open the same file, but clients pass different environments (Codex strips `XDG_DATA_HOME`), so the path comes from an explicit `REMEMBER_DB` written into each Agent's MCP config by `remember setup`, falling back to `$HOME/.local/share/remember/remember.db` with `XDG_DATA_HOME` ignored.
- The Project is resolved from the server's working directory, which Claude Code, Codex and opencode all set to the project directory; MCP roots are not used because Codex does not support them and SEP-2577 deprecates them.
- Commits never fsync: with `synchronous = NORMAL` only a WAL checkpoint does, so each process checkpoints on a background connection once writes pause, and SQLite's own in-commit checkpoint is kept only as a backstop at 4,000 pages.
- Every write transaction starts with `BEGIN IMMEDIATE`: a deferred transaction that reads and then writes gets `SQLITE_BUSY` under WAL without waiting for the busy timeout.
- Clients stop servers by closing stdin or by signal, so the server handles SIGTERM, SIGINT and SIGHUP to mark its Agent offline.
