use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use super::*;
use crate::model::{Category, DAY, Scope};
use crate::{NewMemory, RecallQuery};

const START: i64 = 1_790_000_000;

struct Fixture {
    hub: Hub,
    clock: Arc<AtomicI64>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    fn with_limits(limits: Limits) -> Self {
        let clock = Arc::new(AtomicI64::new(START));
        let tick = clock.clone();
        let hub = Hub::open_in_memory(limits)
            .unwrap()
            .with_clock(move || tick.load(Ordering::SeqCst));
        Fixture { hub, clock }
    }

    fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }

    fn save(
        &mut self,
        agent: &str,
        scope: Scope,
        category: Category,
        key: Option<&str>,
        text: &str,
    ) -> String {
        self.hub
            .remember(
                agent,
                NewMemory {
                    text,
                    scope,
                    category,
                    key,
                },
            )
            .unwrap()
    }

    fn recall(&self, agent: &str, text: Option<&str>) -> Vec<String> {
        self.hub
            .recall(
                agent,
                &RecallQuery {
                    text,
                    ..Default::default()
                },
            )
            .unwrap()
            .memories
            .into_iter()
            .map(|m| m.text)
            .collect()
    }
}

#[test]
fn keyed_memory_replaces_and_keeps_revisions() {
    let mut f = Fixture::new();
    let a = f.hub.register("claude-code", "github.com/x/app").unwrap();
    let id = f.save(
        &a,
        Scope::Project,
        Category::Decision,
        Some("auth.mechanism"),
        "JWT in httpOnly cookie",
    );
    assert_eq!(id, "m1");
    assert_eq!(
        f.save(
            &a,
            Scope::Project,
            Category::Decision,
            Some("auth.mechanism"),
            "JWT in httpOnly cookie"
        ),
        "m1 exists"
    );
    assert_eq!(
        f.save(
            &a,
            Scope::Project,
            Category::Decision,
            Some("auth.mechanism"),
            "server session cookie"
        ),
        "m1 updated"
    );
    assert_eq!(f.recall(&a, Some("auth")), vec!["server session cookie"]);
    let detail = f.hub.show_memory("m1").unwrap();
    assert_eq!(detail.revisions.len(), 1);
    assert_eq!(detail.revisions[0].text, "JWT in httpOnly cookie");

    for n in 0..15 {
        f.save(
            &a,
            Scope::Project,
            Category::Decision,
            Some("auth.mechanism"),
            &format!("variant {n}"),
        );
    }
    assert_eq!(f.hub.show_memory("m1").unwrap().revisions.len(), 10);
}

#[test]
fn same_key_in_different_scopes_is_independent() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "github.com/x/app").unwrap();
    assert_eq!(
        f.save(&a, Scope::User, Category::Fact, Some("pm"), "pnpm"),
        "m1"
    );
    assert_eq!(
        f.save(&a, Scope::Project, Category::Fact, Some("pm"), "cargo"),
        "m2"
    );
}

#[test]
fn unkeyed_exact_duplicate_is_a_no_op() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p").unwrap();
    assert_eq!(
        f.save(&a, Scope::Project, Category::Note, None, "flaky test in ci"),
        "m1"
    );
    assert_eq!(
        f.save(&a, Scope::Project, Category::Note, None, "flaky test in ci"),
        "m1 exists"
    );
    assert_eq!(
        f.save(&a, Scope::Project, Category::Todo, None, "flaky test in ci"),
        "m2"
    );
}

#[test]
fn rejects_invalid_memories() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p").unwrap();
    let err = |r: Result<String>| r.unwrap_err().to_string();
    let long = "x".repeat(1340);
    assert_eq!(
        err(f.hub.remember(
            &a,
            NewMemory {
                text: &long,
                scope: Scope::Project,
                category: Category::Note,
                key: None
            }
        )),
        "memory too long (1340/1000); split or summarize"
    );
    assert_eq!(
        err(f.hub.remember(
            &a,
            NewMemory {
                text: concat!("stripe key is sk_", "live_51H8xY2eZvKYlo2C0aBcDeFgHiJ"),
                scope: Scope::User,
                category: Category::Fact,
                key: None,
            }
        )),
        "looks like a secret (stripe key); store a reference, not the value"
    );
    assert_eq!(
        err(f.hub.remember(
            &a,
            NewMemory {
                text: "x",
                scope: Scope::Task,
                category: Category::Note,
                key: None
            }
        )),
        "no current task; join or start one"
    );
    assert!(
        err(f.hub.remember(
            &a,
            NewMemory {
                text: "x",
                scope: Scope::User,
                category: Category::Note,
                key: Some("has space")
            }
        ))
        .starts_with("invalid key")
    );
}

#[test]
fn recall_respects_visibility() {
    let mut f = Fixture::new();
    let a = f.hub.register("claude-code", "github.com/x/app").unwrap();
    let other = f.hub.register("codex", "github.com/x/other").unwrap();
    f.save(
        &a,
        Scope::User,
        Category::Fact,
        None,
        "user prefers terse replies",
    );
    f.save(
        &a,
        Scope::Project,
        Category::Fact,
        None,
        "app uses postgres replicas",
    );
    f.save(
        &other,
        Scope::Project,
        Category::Fact,
        None,
        "other uses postgres too",
    );
    f.hub.start_task(&a, "migrate db", None).unwrap();
    f.save(
        &a,
        Scope::Task,
        Category::Note,
        None,
        "postgres migration half done",
    );

    let b = f.hub.register("opencode", "github.com/x/app").unwrap();
    // b has not joined the Task, so the Task Memory is hidden.
    let mut seen = f.recall(&b, Some("postgres"));
    seen.sort();
    assert_eq!(seen, vec!["app uses postgres replicas"]);
    assert_eq!(
        f.recall(&other, Some("terse")),
        vec!["user prefers terse replies"]
    );
    assert_eq!(f.recall(&a, Some("postgres")).len(), 2);
}

#[test]
fn recall_ranks_and_tolerates_punctuation() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p").unwrap();
    f.save(
        &a,
        Scope::Project,
        Category::Fact,
        None,
        "cookie banner copy lives in cms",
    );
    f.save(
        &a,
        Scope::Project,
        Category::Decision,
        Some("auth.cookie"),
        "auth token lives in an httpOnly cookie",
    );
    f.save(
        &a,
        Scope::Project,
        Category::Fact,
        None,
        "deploys run on fridays",
    );
    let hits = f.recall(&a, Some("auth: (cookie) \"httpOnly\" -"));
    assert_eq!(hits[0], "auth token lives in an httpOnly cookie");
    assert_eq!(hits.len(), 2);
    let decisions = f
        .hub
        .recall(
            &a,
            &RecallQuery {
                category: Some(Category::Decision),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(decisions.memories.len(), 1);
    let by_key = f
        .hub
        .recall(
            &a,
            &RecallQuery {
                key: Some("auth.cookie"),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(by_key.memories[0].key.as_deref(), Some("auth.cookie"));
    // No query means most recent first.
    assert_eq!(f.recall(&a, None)[0], "deploys run on fridays");
    assert_eq!(f.recall(&a, Some("!!!")).len(), 3);
}

#[test]
fn recall_limit_reports_more() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p").unwrap();
    for n in 0..12 {
        f.save(
            &a,
            Scope::Project,
            Category::Note,
            None,
            &format!("note {n}"),
        );
    }
    let recall = f.hub.recall(&a, &RecallQuery::default()).unwrap();
    assert_eq!(recall.memories.len(), 10);
    assert!(recall.truncated.unwrap().starts_with("more matches"));
}

#[test]
fn crash_safe_handoff_between_agents() {
    let mut f = Fixture::new();
    let claude = f.hub.register("claude-code", "github.com/x/app").unwrap();
    let task = f
        .hub
        .start_task(
            &claude,
            "Refactor auth",
            Some("goal: move auth to middleware"),
        )
        .unwrap();
    assert_eq!((task.task.as_str(), task.brief_revision), ("t1", 1));
    f.save(
        &claude,
        Scope::Task,
        Category::Decision,
        Some("auth.location"),
        "middleware/auth.rs",
    );
    f.hub
        .brief(
            &claude,
            Some("goal: move auth\nstate: middleware written\nnext: wire routes"),
            Some(1),
        )
        .unwrap();
    // Claude Code dies here without calling anything else.

    f.advance(3600);
    let codex = f
        .hub
        .register("codex-mcp-client", "github.com/x/app")
        .unwrap();
    let context = f.hub.context(&codex, None).unwrap();
    assert_eq!(context.tasks.len(), 1);
    assert_eq!(context.tasks[0].last_agent, claude);
    assert_eq!(context.tasks[0].age, "1h");

    let joined = f.hub.join_task(&codex, "t1").unwrap();
    assert_eq!(joined.brief_revision, 2);
    assert!(joined.brief.contains("next: wire routes"));
    assert_eq!(joined.memories[0].text, "middleware/auth.rs");

    f.hub
        .brief(&codex, Some("state: routes wired"), Some(2))
        .unwrap();
    let stale = f
        .hub
        .brief(&claude, Some("state: middleware written"), Some(2))
        .unwrap_err()
        .to_string();
    assert!(
        stale.starts_with("brief changed since revision 2"),
        "{stale}"
    );
    assert!(stale.contains("brief_revision: 3"));
    assert!(stale.contains("routes wired"));
}

#[test]
fn closing_promotes_and_archives() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p").unwrap();
    f.save(
        &a,
        Scope::Project,
        Category::Decision,
        Some("refresh.location"),
        "refresh in controller",
    );
    f.hub.start_task(&a, "token refresh", None).unwrap();
    let durable = f.save(
        &a,
        Scope::Task,
        Category::Decision,
        Some("refresh.location"),
        "refresh lives in middleware X",
    );
    let fresh = f.save(
        &a,
        Scope::Task,
        Category::Fact,
        None,
        "refresh interval is 15m",
    );
    f.save(
        &a,
        Scope::Task,
        Category::Note,
        None,
        "tried a cron approach, dropped it",
    );

    let closed = f
        .hub
        .close_task(&a, None, "refresh moved to middleware", &[durable, fresh])
        .unwrap();
    assert_eq!(closed, "t1 done, promoted 2");

    let recalled = f.recall(&a, Some("refresh"));
    assert!(recalled.contains(&"refresh lives in middleware X".to_string()));
    assert!(recalled.contains(&"refresh interval is 15m".to_string()));
    assert!(!recalled.contains(&"tried a cron approach, dropped it".to_string()));
    assert!(!recalled.contains(&"refresh in controller".to_string()));
    // The clashing Project Memory kept its id and gained a Revision.
    assert_eq!(
        f.hub.show_memory("m1").unwrap().revisions[0].text,
        "refresh in controller"
    );

    let archived = f
        .hub
        .recall(
            &a,
            &RecallQuery {
                text: Some("cron"),
                include_done: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(archived.memories[0].scope, "t1");

    assert_eq!(
        f.hub.join_task(&a, "t1").unwrap_err().to_string(),
        "task t1 is done; reopen it first"
    );
    let reopened = f.hub.reopen_task(&a, "t1").unwrap();
    assert_eq!(reopened.status, "open");
    assert_eq!(reopened.memories.len(), 1);
}

#[test]
fn close_rejects_foreign_promotions() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p").unwrap();
    let project_memory = f.save(&a, Scope::Project, Category::Fact, None, "unrelated");
    f.hub.start_task(&a, "t", None).unwrap();
    let err = f
        .hub
        .close_task(&a, None, "done", &[project_memory])
        .unwrap_err();
    assert_eq!(err.to_string(), "m1 is not a memory of task t1");
    assert_eq!(f.hub.list_tasks(&a, false).unwrap().tasks[0].status, "open");
}

#[test]
fn tasks_go_stale_but_stay_open() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p").unwrap();
    f.hub.start_task(&a, "old work", None).unwrap();
    f.advance(15 * DAY);
    let tasks = f.hub.list_tasks(&a, false).unwrap().tasks;
    assert_eq!(
        (tasks[0].status.as_str(), tasks[0].age.as_str()),
        ("stale", "15d")
    );
}

#[test]
fn joining_another_task_leaves_the_current_one() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p").unwrap();
    f.hub.start_task(&a, "first", None).unwrap();
    f.hub.start_task(&a, "second", None).unwrap();
    f.save(&a, Scope::Task, Category::Note, None, "belongs to second");
    let first = f.hub.join_task(&a, "t1").unwrap();
    assert!(first.memories.is_empty());
    assert_eq!(
        f.hub.context(&a, None).unwrap().current_task.as_deref(),
        Some("t1")
    );
    f.hub.leave_task(&a).unwrap();
    assert_eq!(
        f.hub.leave_task(&a).unwrap_err().to_string(),
        "no current task"
    );
}

#[test]
fn tasks_are_scoped_to_their_project() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p1").unwrap();
    let b = f.hub.register("codex", "p2").unwrap();
    f.hub.start_task(&a, "p1 work", None).unwrap();
    assert_eq!(
        f.hub.join_task(&b, "t1").unwrap_err().to_string(),
        "task t1 not found in this project"
    );
}

#[test]
fn messages_to_agents_require_them_online() {
    let mut f = Fixture::new();
    let a = f.hub.register("claude-code", "p").unwrap();
    let b = f.hub.register("codex", "p").unwrap();
    f.hub.send(&a, &b, "take the frontend").unwrap();
    assert_eq!(f.hub.unread_count(&b).unwrap(), 1);
    assert_eq!(f.hub.unread_count(&a).unwrap(), 0);
    let inbox = f.hub.inbox(&b).unwrap();
    assert_eq!(inbox.messages[0].from, a);
    assert_eq!(inbox.messages[0].text, "take the frontend");
    assert_eq!(f.hub.unread_count(&b).unwrap(), 0);

    f.advance(11 * 60);
    assert_eq!(
        f.hub.send(&a, &b, "still there?").unwrap_err().to_string(),
        "agent offline; address the task instead"
    );
    f.hub.touch(&b).unwrap();
    f.hub.send(&a, &b, "still there?").unwrap();
    f.hub.end(&b).unwrap();
    assert!(f.hub.send(&a, &b, "bye").is_err());
    assert_eq!(
        f.hub.send(&a, &a, "me").unwrap_err().to_string(),
        "cannot message yourself; use remember"
    );
    assert_eq!(
        f.hub.send(&a, "ghost#0000", "hi").unwrap_err().to_string(),
        "agent ghost#0000 not found"
    );
}

#[test]
fn task_messages_reach_later_joiners() {
    let mut f = Fixture::new();
    let a = f.hub.register("claude-code", "p").unwrap();
    f.hub.start_task(&a, "shared", None).unwrap();
    f.hub
        .send(&a, "t1", "careful: tests hit the real db")
        .unwrap();
    f.hub.end(&a).unwrap();

    let b = f.hub.register("codex", "p").unwrap();
    assert_eq!(f.hub.unread_count(&b).unwrap(), 0);
    f.hub.join_task(&b, "t1").unwrap();
    assert_eq!(f.hub.unread_count(&b).unwrap(), 1);
    assert_eq!(f.hub.inbox(&b).unwrap().messages[0].to, "t1");

    let c = f.hub.register("opencode", "p").unwrap();
    f.hub.join_task(&c, "t1").unwrap();
    assert_eq!(
        f.hub.unread_count(&c).unwrap(),
        1,
        "each Agent tracks its own read state"
    );
}

#[test]
fn prune_applies_retention() {
    let mut f = Fixture::new();
    let a = f.hub.register("claude-code", "p").unwrap();
    let b = f.hub.register("codex", "p").unwrap();
    f.hub.send(&a, &b, "read soon").unwrap();
    f.hub.inbox(&b).unwrap();
    f.hub.send(&a, &b, "never read").unwrap();
    f.hub.start_task(&a, "t", None).unwrap();
    f.hub.send(&a, "t1", "task note").unwrap();
    f.hub.close_task(&a, None, "done", &[]).unwrap();

    f.advance(8 * DAY);
    f.hub.touch(&a).unwrap();
    f.hub.prune().unwrap();
    let count = |hub: &Hub| -> i64 {
        hub.conn
            .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(
        count(&f.hub),
        1,
        "agent messages go after 7 days; task messages stay"
    );

    f.advance(30 * DAY);
    f.hub.prune().unwrap();
    assert_eq!(count(&f.hub), 0);
}

#[test]
fn context_respects_budget() {
    let mut f = Fixture::with_limits(Limits {
        context_tokens: 300,
        ..Limits::default()
    });
    let a = f.hub.register("codex", "p").unwrap();
    f.save(
        &a,
        Scope::User,
        Category::Fact,
        Some("style"),
        "terse replies",
    );
    for n in 0..40 {
        f.save(
            &a,
            Scope::Project,
            Category::Decision,
            Some(&format!("d{n}")),
            &format!("decision number {n} about the system"),
        );
    }
    let context = f.hub.context(&a, Some("backend")).unwrap();
    assert_eq!(context.user.len(), 1);
    assert!(context.decisions.len() < 40);
    assert_eq!(
        context.decisions[0].key.as_deref(),
        Some("d39"),
        "newest first"
    );
    let truncated = context.truncated.clone().unwrap();
    assert!(truncated.contains("more decisions"), "{truncated}");
    let encoded = crate::toon::encode(&context).unwrap();
    assert!(crate::model::estimate_tokens(&encoded) <= 300, "{encoded}");
}

#[test]
fn forget_is_limited_to_visible_memories() {
    let mut f = Fixture::new();
    let a = f.hub.register("codex", "p1").unwrap();
    let b = f.hub.register("codex", "p2").unwrap();
    let id = f.save(&a, Scope::Project, Category::Fact, None, "p1 only");
    assert_eq!(
        f.hub.forget(&b, &id).unwrap_err().to_string(),
        "m1 not found"
    );
    f.hub.forget(&a, &id).unwrap();
    assert!(f.recall(&a, None).is_empty());
}
