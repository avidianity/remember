//! Runs the vendored official TOON encode fixtures through our encoder seam.
//! Only default-option cases apply: Remember always encodes with the defaults.

use std::fs;
use std::path::Path;

#[test]
fn encoder_matches_spec_fixtures() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/toon-encode");
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let suite: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        for case in suite["tests"].as_array().unwrap() {
            if case
                .get("options")
                .is_some_and(|o| !o.as_object().unwrap().is_empty())
            {
                continue;
            }
            checked += 1;
            let name = case["name"].as_str().unwrap();
            let actual = remember_core::toon::encode(&case["input"]);
            match (
                case.get("shouldError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                actual,
            ) {
                (true, Ok(out)) => failures.push(format!("{name}: expected error, got {out:?}")),
                (false, Err(e)) => failures.push(format!("{name}: {e}")),
                (false, Ok(out)) if out != case["expected"].as_str().unwrap() => {
                    failures.push(format!("{name}: got {out:?}, want {:?}", case["expected"]))
                }
                _ => {}
            }
        }
    }
    assert!(checked >= 150, "only {checked} fixtures ran");
    assert!(
        failures.is_empty(),
        "{} of {checked} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
