//! Latency budget: p99 under 10ms per operation on a DB with 50k Memories.
//! Run in release: `cargo test --release -p remember-core --test perf -- --ignored`.

use std::time::{Duration, Instant};

use remember_core::{Category, Hub, Limits, NewMemory, RecallQuery, Scope};

/// Deterministic pseudo-random words with a Zipf-like skew: a few words are
/// very common, most are rare, as in real notes.
struct Corpus {
    state: u64,
}

impl Corpus {
    fn next(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 11) as f64 / (1u64 << 53) as f64
    }

    fn word(&mut self) -> String {
        let rank = (2000.0 * self.next().powi(3)) as usize;
        let syllables = ["ka", "lo", "mi", "ne", "ru", "ta", "vo", "zi"];
        let mut word = String::new();
        let mut n = rank;
        loop {
            word.push_str(syllables[n % 8]);
            n /= 8;
            if n == 0 {
                break;
            }
        }
        word + "x"
    }

    fn sentence(&mut self, words: usize) -> String {
        (0..words)
            .map(|_| self.word())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn p99(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() * 99 / 100]
}

#[test]
#[ignore = "benchmark; run in release"]
fn p99_under_10ms_at_50k_memories() {
    let tmp = tempfile::tempdir().unwrap();
    let mut hub = Hub::open(&tmp.path().join("remember.db"), Limits::default()).unwrap();
    let agent = hub.register("bench", "github.com/acme/app").unwrap();
    hub.start_task(&agent, "bench task", None).unwrap();
    let mut corpus = Corpus { state: 42 };
    for n in 0..50_000usize {
        let text = corpus.sentence(12);
        let scope = [Scope::Project, Scope::Task, Scope::User][n % 3];
        let key = (n % 5 == 0).then(|| format!("k{n}"));
        hub.remember(
            &agent,
            NewMemory {
                text: &text,
                scope,
                category: Category::Decision,
                key: key.as_deref(),
            },
        )
        .unwrap();
    }

    println!("seeded 50k");
    let measure = |name: &str, f: &mut dyn FnMut(usize)| {
        let samples: Vec<Duration> = (0..300)
            .map(|i| {
                let start = Instant::now();
                f(i);
                start.elapsed()
            })
            .collect();
        let p = p99(samples);
        println!("{name}: p99 {p:?}");
        assert!(p < Duration::from_millis(10), "{name} p99 {p:?}");
    };
    let hub = std::cell::RefCell::new(hub);
    let queries: Vec<String> = (0..300).map(|_| corpus.sentence(2)).collect();
    measure("recall query", &mut |i| {
        hub.borrow()
            .recall(
                &agent,
                &RecallQuery {
                    text: Some(&queries[i]),
                    ..Default::default()
                },
            )
            .unwrap();
    });
    measure("recall recent", &mut |_| {
        hub.borrow()
            .recall(&agent, &RecallQuery::default())
            .unwrap();
    });
    measure("context", &mut |_| {
        hub.borrow().context(&agent, None).unwrap();
    });
    measure("remember keyed", &mut |i| {
        let text = format!("updated value {i}");
        hub.borrow_mut()
            .remember(
                &agent,
                NewMemory {
                    text: &text,
                    scope: Scope::Project,
                    category: Category::Fact,
                    key: Some("hot"),
                },
            )
            .unwrap();
    });
    measure("unread count", &mut |_| {
        hub.borrow().unread_count(&agent).unwrap();
    });
}
