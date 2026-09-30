# Remember

Remember is a shared memory and messaging hub for coding agents from different vendors.
It exists so work can be handed off between agents (Claude Code, opencode, Codex, and others) without losing context.

## Language

### Participants

**Agent**:
One running instance of a coding agent, registered with Remember for the lifetime of its process.
_Avoid_: Client, bot, assistant

**Agent Kind**:
The product an Agent is an instance of, such as `claude-code`, `codex`, or `opencode`.
_Avoid_: Agent type, vendor

**Registration**:
The automatic recording of an Agent, with its Agent Kind and Project, the moment it connects to Remember.
_Avoid_: Login, signup

**Run**:
A vendor's own conversation or session inside one Agent; Remember does not model it.
_Avoid_: Session

### Work

**Project**:
One codebase, identified by its git remote URL when present, otherwise its git root path.
_Avoid_: Repo, workspace

**Task**:
A unit of work inside a Project that one or more Agents join, possibly one after another, to continue the same effort.
_Avoid_: Session, thread, job

**Current Task**:
The one Task an Agent has joined; an Agent has at most one, and joining another leaves the previous one.
_Avoid_: Active task, selected task

**Brief**:
The single living summary of a Task (goal, current state, next steps, open questions) that Agents keep current as they work.
_Avoid_: Handoff note, summary, status

**Open Task**:
A Task still being worked on; it shows as stale after 14 days without activity but stays open.

**Done Task**:
A Task that was closed with a one-line outcome; its Task Memories leave default recall but remain searchable, and it can be reopened.
_Avoid_: Archived, deleted

**Handoff**:
One Agent leaving a Task so another Agent can join it and continue.
_Avoid_: Switch, transfer

### Memory

**Memory**:
A piece of knowledge an Agent saves so that it or other Agents can recall it later.
_Avoid_: Note, fact, record

**Key**:
An optional short name on a Memory; saving a Memory with the same Key in the same Scope replaces the current one.
_Avoid_: ID, slug, title

**Revision**:
A replaced earlier version of a keyed Memory, kept as history but never recalled by default.
_Avoid_: Version, snapshot

**Category**:
One fixed label per Memory: `fact`, `decision`, `todo`, or `note`.
_Avoid_: Tag, type, kind

**Scope**:
The visibility boundary of a Memory: User, Project, or Task.
_Avoid_: Namespace, level, type

**User Scope**:
Memories visible to every Agent in every Project, holding facts about the human and their preferences.
_Avoid_: Global (ambiguous)

**Project Scope**:
Memories visible to every Agent working in one Project, across all its Tasks.

**Task Scope**:
Memories visible only to Agents that have joined one Task.
_Avoid_: Session memory

**Promotion**:
Moving a Task Memory to Project Scope, usually when closing the Task, because it stays true beyond that Task.
_Avoid_: Export, publish

### Messaging

**Message**:
Text an Agent addresses either to one online Agent or to a Task, where every Agent in or later joining that Task receives it.
_Avoid_: Chat, notification

**Online Agent**:
An Agent whose process is running and that made a tool call within the last 10 minutes; any other Agent is offline and cannot receive Messages.

**Inbox**:
The Messages addressed to one Agent that it has not yet read.

**Wait**:
An Agent blocking on its Inbox for a bounded time until a Message arrives, so two working Agents can converse turn by turn.
_Avoid_: Subscribe, listen

**Wake**:
Prompting an idle Online Agent to read its Inbox by typing a fixed nudge into the terminal it runs in; only Agents running inside tmux can be woken.
_Avoid_: Ping, push, notify
