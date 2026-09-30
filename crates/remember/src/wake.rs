//! Waking an idle Agent by typing a one-line nudge into its tmux pane.
//!
//! MCP clients never show server-initiated messages to the model, so an Agent
//! waiting at its prompt only reacts to typed input. Only a fixed nudge is
//! typed, never the Message text, so no sender can inject keystrokes; the
//! Message itself is read through `inbox`.

use std::process::{Command, Stdio};

use remember_core::Terminal;

/// Returns how many recipients were nudged.
pub fn wake(targets: &[Terminal], from: &str) -> usize {
    let nudge = format!("remember: new message from {from}, call the inbox tool");
    targets.iter().filter(|t| wake_one(t, &nudge)).count()
}

fn wake_one(terminal: &Terminal, nudge: &str) -> bool {
    let Some(pane) = &terminal.tmux_pane else {
        return false;
    };
    // The pane may outlive the Agent (e.g. it crashed back to a shell); never
    // type into a pane whose Agent process is gone.
    if !alive(terminal.pid) {
        return false;
    }
    let tmux = |args: &[&str]| {
        let mut command = Command::new("tmux");
        if let Some(socket) = &terminal.tmux_socket {
            command.args(["-S", socket]);
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    tmux(&["send-keys", "-t", pane, "-l", nudge]) && tmux(&["send-keys", "-t", pane, "Enter"])
}

fn alive(pid: i64) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// This process's Agent: our parent, plus the tmux pane it runs in, if any.
pub fn current_terminal() -> Option<Terminal> {
    #[cfg(unix)]
    {
        let pid = i64::from(std::os::unix::process::parent_id());
        // TMUX is "<socket>,<server pid>,<session>".
        let socket = std::env::var("TMUX")
            .ok()
            .and_then(|v| v.split(',').next().map(str::to_string))
            .filter(|s| !s.is_empty());
        let pane = std::env::var("TMUX_PANE").ok().filter(|p| !p.is_empty());
        Some(Terminal {
            pid,
            tmux_socket: socket,
            tmux_pane: pane,
        })
    }
    #[cfg(not(unix))]
    {
        None
    }
}
