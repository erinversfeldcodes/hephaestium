//! A gate result that says it was left to a CI check: only for a gate that names one,
//! never a pass, and a result the contract accepts.

mod common;

use std::collections::BTreeMap;

use common::conforms;
use heph::gate::{self, Outcome};
use heph::manifest::Gate;

fn gate_with(ci_check: Option<&str>) -> Gate {
    Gate {
        run: vec!["just".to_owned(), "test".to_owned()],
        when_paths: None,
        timeout_seconds: 60,
        result: None,
        requires: BTreeMap::new(),
        ci_check: ci_check.map(str::to_owned),
    }
}

fn command() -> Vec<String> {
    vec!["just".to_owned(), "test".to_owned()]
}

#[test]
fn a_gate_that_names_no_check_cannot_be_deferred() {
    let result = gate::deferred("test", &gate_with(None), command(), 0, "not set".to_owned());

    assert!(result.is_none());
}

#[test]
fn a_deferred_result_names_the_check_and_why_has_no_exit_code_and_conforms_to_the_contract() {
    let result = gate::deferred(
        "unit",
        &gate_with(Some("ci / test")),
        command(),
        1500,
        "timed out after 60s".to_owned(),
    )
    .expect("a deferral");

    assert_eq!(result.outcome, Outcome::Deferred);
    assert_eq!(result.ci_check.as_deref(), Some("ci / test"));
    assert_eq!(result.reason.as_deref(), Some("timed out after 60s"));
    assert_eq!(result.exit_code, None);
    assert_eq!(result.duration_ms, 1500);
    let json = serde_json::to_value(&result).expect("JSON");
    assert_eq!(json["outcome"], "deferred");
    assert_eq!(json["ci_check"], "ci / test");
    assert!(json.get("exit_code").is_none(), "{json}");
    conforms(&json);
}

#[test]
fn a_deferral_is_not_a_pass_for_heph_itself() {
    let result = gate::deferred(
        "test",
        &gate_with(Some("test")),
        command(),
        0,
        "x".to_owned(),
    )
    .expect("a deferral");

    assert_eq!(result.exit_code_of_heph(), 1);
    assert_eq!(Outcome::Deferred.to_string(), "deferred");
}
