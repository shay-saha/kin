use kin_brain::keepers::{Keeper, KeeperInput, build_keeper_result};
use serde::Deserialize;

#[derive(Deserialize)]
struct Input {
    keeper: Keeper,
    input: KeeperInput,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    input: Input,
    expected: serde_json::Value,
}

#[derive(Deserialize)]
struct Fixtures {
    cases: Vec<Case>,
}

const EPS: f64 = 1e-9;

#[test]
fn matches_typescript_keeper() {
    let raw = include_str!("../../conformance/keeper-fixtures.json");
    let fixtures: Fixtures = serde_json::from_str(raw).expect("fixtures parse");
    assert!(!fixtures.cases.is_empty(), "no fixtures loaded");

    let mut failures: Vec<String> = Vec::new();
    let mut claims = 0usize;

    for case in &fixtures.cases {
        let got = build_keeper_result(&case.input.keeper, &case.input.input);
        let got = serde_json::to_value(&got).expect("serialize");
        let want = &case.expected;
        for key in ["keeperId", "support", "evidence"] {
            assert_eq!(got[key], want[key], "{}: {} differs", case.name, key);
        }

        if !want["claim"].is_null() {
            claims += 1;
        }
        if got["claim"] != want["claim"] {
            failures.push(format!(
                "{}: claim {} != {}",
                case.name, got["claim"], want["claim"]
            ));
        }
        if got["reason"] != want["reason"] {
            failures.push(format!(
                "{}: reason {} != {}",
                case.name, got["reason"], want["reason"]
            ));
        }
        if got["memoryIds"] != want["memoryIds"] {
            failures.push(format!(
                "{}: memoryIds {} != {}",
                case.name, got["memoryIds"], want["memoryIds"]
            ));
        }
        for key in ["v", "r"] {
            let g = got[key].as_f64().expect("signal");
            let w = want[key].as_f64().expect("signal");
            if (g - w).abs() > EPS {
                failures.push(format!("{}: {} {} != {}", case.name, key, g, w));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} cases drifted:\n  {}",
        failures.len(),
        fixtures.cases.len(),
        failures.join("\n  ")
    );
    eprintln!(
        "keeper conformance: {} cases ({} claim, {} abstain)",
        fixtures.cases.len(),
        claims,
        fixtures.cases.len() - claims
    );
}
