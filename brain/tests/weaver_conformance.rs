use kin_brain::weaver::{WeaverData, find_gaps, pick_top_gap, route_question};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    input: WeaverData,
    expected: serde_json::Value,
}

#[derive(Deserialize)]
struct Fixtures {
    cases: Vec<Case>,
}

#[test]
fn matches_typescript_weaver() {
    let raw = include_str!("../../conformance/weaver-fixtures.json");
    let fixtures: Fixtures = serde_json::from_str(raw).expect("fixtures parse");
    assert!(!fixtures.cases.is_empty(), "no fixtures loaded");

    let mut failures: Vec<String> = Vec::new();

    for case in &fixtures.cases {
        let gaps = serde_json::to_value(find_gaps(&case.input)).expect("serialize gaps");
        if gaps != case.expected["gaps"] {
            failures.push(format!(
                "{}: gaps {} != {}",
                case.name, gaps, case.expected["gaps"]
            ));
        }

        let top = pick_top_gap(&case.input);
        let top_json = serde_json::to_value(&top).expect("serialize top");
        if top_json != case.expected["topGap"] {
            failures.push(format!(
                "{}: topGap {} != {}",
                case.name, top_json, case.expected["topGap"]
            ));
        }

        let routed = top.as_ref().and_then(|g| route_question(&case.input, g));
        let routed_json = serde_json::to_value(&routed).expect("serialize route");
        if routed_json != case.expected["routedTo"] {
            failures.push(format!(
                "{}: routedTo {} != {}",
                case.name, routed_json, case.expected["routedTo"]
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} drifts across {} cases:\n  {}",
        failures.len(),
        fixtures.cases.len(),
        failures.join("\n  ")
    );
    eprintln!("weaver conformance: {} graphs", fixtures.cases.len());
}
