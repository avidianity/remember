# Live messaging by long-poll Wait and tmux Wake, not server push

Agents converse live through two mechanisms instead of server push.
An Agent that expects a reply calls `inbox` with `wait`, which blocks until a Message arrives or the wait ends.
A sender can also pass `wake` on `send`, which types a fixed one-line nudge into each recipient's tmux pane so an Agent idling at its prompt starts a turn and reads its Inbox.

We rejected server push because no target client shows a server-initiated message to the model, except Claude Code's channels, which are opt-in per launch and unique to one client.
We rejected a daemon relaying Messages because ADR 0001 keeps Remember daemonless; each server notices Messages from other processes by polling the shared file every 250 ms, an indexed count that costs microseconds.

## Consequences

- The wait is capped at 50 seconds, below the 60-second tool-call timeout of Claude Code (2.1.285), Codex (0.150) and opencode (1.18), each verified with a real 50-second wait. opencode's 5-second MCP timeout covers only listing tools, not calling them. An Agent that needs longer calls `inbox` again.
- A cancelled tool call ends the wait at once, so an interrupted Agent is never stuck.
- Wake types only a fixed nudge naming the sender, never the Message text, so no Agent can inject keystrokes into another Agent's terminal. The nudge is typed literally with `send-keys -l`.
- Wake only reaches Agents running inside tmux, found from the `TMUX` and `TMUX_PANE` variables at Registration. Codex strips these from the server's environment, so `remember setup codex` asks the user to forward them with `env_vars`.
- A pane can outlive its Agent (it crashed back to a shell), so Wake first checks that the Agent's client process is still alive and never types into a pane whose Agent is gone.
- Wake is opt-in per `send`, because typing into a pane interrupts a human who might be using it.
