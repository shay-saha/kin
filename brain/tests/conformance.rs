use kin_brain::gate::{GateInfo, KeeperResult, evaluate_gate};
use serde::Deserialize;

#[derive(Deserialize)]
struct Input {
    results: Vec<KeeperResult>,
    info: GateInfo,
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
fn matches_typescript_gate() {
    let raw = include_str!("../../conformance/gate-fixtures.json");
    let fixtures: Fixtures = serde_json::from_str(raw).expect("fixtures parse");
    assert!(!fixtures.cases.is_empty(), "no fixtures loaded");

    let mut failures: Vec<String> = Vec::new();
    let mut speak = 0usize;

    for case in &fixtures.cases {
        let got = evaluate_gate(&case.input.results, &case.input.info);
        let want = &case.expected;

        let got_json = serde_json::to_value(&got).expect("serialize result");
        assert_eq!(
            got_json["reasonCode"], want["reasonCode"],
            "{}: reasonCode differs",
            case.name
        );

        let want_decision = want["decision"].as_str().expect("decision");
        let got_decision = got_json["decision"].as_str().expect("decision");
        if want_decision == "speak" {
            speak += 1;
        }
        if got_decision != want_decision {
            failures.push(format!(
                "{}: decision {} != {}",
                case.name, got_decision, want_decision
            ));
            continue;
        }

        let want_reason = want["reason"].as_str().expect("reason");
        let got_reason = got_json["reason"].as_str().expect("reason");
        if got_reason != want_reason {
            failures.push(format!(
                "{}: reason {:?} != {:?}",
                case.name, got_reason, want_reason
            ));
        }

        for key in ["V", "R", "A", "S", "X", "C", "threshold"] {
            let w = want[key].as_f64().expect("signal");
            let g = got_json[key].as_f64().expect("signal");
            if (w - g).abs() > EPS {
                failures.push(format!("{}: {} {} != {}", case.name, key, g, w));
            }
        }

        let list = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        for key in ["agreeingKeeperIds", "citedMemoryIds"] {
            if list(&got_json[key]) != list(&want[key]) {
                failures.push(format!("{}: {} differs", case.name, key));
            }
        }
        if got_json["subjectNodeId"] != want["subjectNodeId"] {
            failures.push(format!("{}: subjectNodeId differs", case.name));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} cases drifted from the TypeScript gate:\n  {}",
        failures.len(),
        fixtures.cases.len(),
        failures.join("\n  ")
    );

    eprintln!(
        "gate conformance: {} cases ({} speak, {} silent)",
        fixtures.cases.len(),
        speak,
        fixtures.cases.len() - speak
    );
}
