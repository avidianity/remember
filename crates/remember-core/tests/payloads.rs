//! Snapshots of the exact TOON Agents receive, so any change to payload shape
//! or size shows up in review.

use remember_core::{Category, Hub, Limits, NewMemory, RecallQuery, Scope, toon};

const NOW: i64 = 1_790_000_000;

fn hub() -> (Hub, String) {
    let mut hub = Hub::open_in_memory(Limits::default())
        .unwrap()
        .with_clock(|| NOW);
    let agent = hub.register("claude-code", "github.com/acme/app").unwrap();
    let save = |hub: &mut Hub, scope, category, key, text| {
        hub.remember(
            &agent,
            NewMemory {
                text,
                scope,
                category,
                key,
            },
        )
        .unwrap();
    };
    save(
        &mut hub,
        Scope::User,
        Category::Fact,
        Some("style"),
        "Reply tersely; JSON output must be TOON.",
    );
    save(
        &mut hub,
        Scope::Project,
        Category::Decision,
        Some("db"),
        "SQLite in WAL mode, one shared file.",
    );
    save(
        &mut hub,
        Scope::Project,
        Category::Fact,
        None,
        "CI runs on GitHub Actions, ubuntu-latest.",
    );
    hub.start_task(
        &agent,
        "Refactor auth",
        Some("goal: auth in middleware\nnext: wire routes"),
    )
    .unwrap();
    save(
        &mut hub,
        Scope::Task,
        Category::Todo,
        None,
        "Wire /api/v2 routes, then delete the old guard.",
    );
    (hub, agent)
}

/// Agent ids carry a random suffix; pin it for stable snapshots.
fn stable(text: String, agent: &str) -> String {
    text.replace(agent, "claude-code#0000")
}

#[test]
fn context_payload() {
    let (hub, agent) = hub();
    let context = toon::encode(&hub.context(&agent, None).unwrap()).unwrap();
    insta::assert_snapshot!(stable(context, &agent));
}

#[test]
fn join_payload() {
    let (mut hub, agent) = hub();
    let joined = toon::encode(&hub.join_task(&agent, "t1").unwrap()).unwrap();
    insta::assert_snapshot!(stable(joined, &agent));
}

#[test]
fn recall_payload() {
    let (hub, agent) = hub();
    let recall = hub
        .recall(
            &agent,
            &RecallQuery {
                text: Some("routes sqlite"),
                ..Default::default()
            },
        )
        .unwrap();
    insta::assert_snapshot!(stable(toon::encode(&recall).unwrap(), &agent));
}
