//! A gate the workspace cannot run, that the repository says a check on the pull
//! request exercises too, is left to that check: the workload still opens its pull
//! request, and says what was not run and why. A gate that ran and failed is a
//! failure whatever check names it, and a gate that names no check is never left.

mod common;

use std::time::{Duration, Instant};

use common::{IDENTITY, agent_script, finished, gh_was_called, manifest, rig, run};
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::{Event, GateFinished};
use serde_json::{Value, json};

/// A variable no machine that runs these tests has set.
const ABSENT: &str = "HEPH_TEST_DEFERRED_NEEDS";

fn gate(name: &str, command: &str, timeout: u64, more: &str) -> String {
    format!(
        "\n[gates.{name}]\nrun = [\"sh\", \"-c\", {command:?}]\ntimeout_seconds = {timeout}\n{more}"
    )
}

/// A gate that needs a variable that is not set, and names the check that covers it.
fn needs_what_is_absent(name: &str, ci_check: Option<&str>) -> String {
    let check = ci_check.map_or(String::new(), |check| format!("ci_check = {check:?}\n"));
    gate(name, "exit 0", 30, &check) + &format!("\n[gates.{name}.requires]\n{ABSENT} = \"env\"\n")
}

fn gate_results(events: &[Event]) -> Vec<(String, Value)> {
    events
        .iter()
        .filter_map(|event| match event.kind.as_ref() {
            Some(Kind::GateFinished(GateFinished { name, result })) => Some((
                name.clone(),
                serde_json::to_value(result.as_ref().expect("a result")).expect("JSON"),
            )),
            _ => None,
        })
        .collect()
}

fn result_of<'a>(results: &'a [(String, Value)], name: &str) -> &'a Value {
    &results
        .iter()
        .find(|(gate, _)| gate == name)
        .unwrap_or_else(|| panic!("no result for {name}: {results:?}"))
        .1
}

fn delivered(name: &str, gates: &str) -> (common::Ran, String) {
    let (ran, _, _) = run(
        rig(name, &(manifest(&[]) + gates)),
        &agent_script(""),
        true,
        0,
        &IDENTITY,
    );
    let body = std::fs::read_to_string(ran.scratch.path("seen/gh-body.txt")).unwrap_or_default();
    (ran, body)
}

#[test]
fn an_unmet_requirement_on_a_gate_with_a_ci_check_defers_it_and_the_pull_request_opens() {
    let (ran, body) = delivered(
        "defer-unmet",
        &needs_what_is_absent("suite", Some("ci / test")),
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Completed, "{}", ran.stderr());
    assert!(gh_was_called(&ran), "no pull request was opened");
    let results = gate_results(&events);
    let result = result_of(&results, "suite");
    assert_eq!(result["outcome"], json!("deferred"), "{result}");
    assert_eq!(result["ci_check"], json!("ci / test"));
    assert!(
        result["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(&format!("{ABSENT} (environment variable)"))),
        "{result}"
    );
    assert!(
        result.get("exit_code").is_none(),
        "a deferred gate has no exit code: {result}"
    );
    // The same result is in the record, which is what the control plane reads.
    let record = ran.record();
    let recorded = record
        .gates
        .iter()
        .find(|run| run.name == "suite")
        .expect("the gate is in the record");
    let recorded = serde_json::to_value(recorded.result.as_ref().expect("a result")).expect("JSON");
    assert_eq!(recorded["outcome"], json!("deferred"), "{recorded}");
    assert_eq!(recorded["ci_check"], json!("ci / test"));
    // And the pull request says it, with why and which check covers it.
    assert!(
        body.contains("Gates not run in the workspace"),
        "the pull request does not say what was not run:\n{body}"
    );
    assert!(body.contains("`suite`"), "{body}");
    assert!(body.contains(ABSENT), "{body}");
    assert!(body.contains("check `ci / test`"), "{body}");
    assert!(
        body.contains("Refs #"),
        "the body lost its closing line:\n{body}"
    );
}

#[test]
fn a_gate_that_names_no_ci_check_is_never_deferred() {
    let (ran, body) = delivered("defer-none", &needs_what_is_absent("test", None));

    let events = ran.events();
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed, "{}", ran.stderr());
    assert!(
        reason.contains("gate test could not be run") && reason.contains(ABSENT),
        "{reason}"
    );
    assert!(!gh_was_called(&ran), "a pull request was opened:\n{body}");
}

#[test]
fn a_gate_that_ran_and_failed_is_a_failure_whatever_check_names_it() {
    let (ran, _) = delivered(
        "defer-failed",
        &gate("test", "exit 3", 30, "ci_check = \"test\"\n"),
    );

    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Failed, "{}", ran.stderr());
    let results = gate_results(&events);
    assert_eq!(result_of(&results, "test")["outcome"], json!("failed"));
    assert!(!gh_was_called(&ran));
}

#[test]
fn a_gate_that_timed_out_and_names_a_ci_check_is_deferred_and_one_that_does_not_fails() {
    let started = Instant::now();
    let (ran, body) = delivered(
        "defer-timeout",
        &gate("slow", "sleep 30", 1, "ci_check = \"slow\"\n"),
    );

    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the gate was not stopped"
    );
    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Completed, "{}", ran.stderr());
    let results = gate_results(&events);
    let result = result_of(&results, "slow");
    assert_eq!(result["outcome"], json!("deferred"), "{result}");
    assert!(
        result["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("timed out after 1s")
                && reason.contains("the load average on this machine")),
        "{result}"
    );
    assert!(body.contains("check `slow`"), "{body}");

    let (ran, _) = delivered("defer-timeout-none", &gate("slow", "sleep 30", 1, ""));

    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Failed, "{}", ran.stderr());
    assert!(
        finished(&events)
            .1
            .contains("the load average on this machine"),
        "{}",
        finished(&events).1
    );
    assert_eq!(
        result_of(&gate_results(&events), "slow")["outcome"],
        json!("timed_out")
    );
}

#[test]
fn a_deferred_gate_does_not_stop_the_others_and_a_gate_that_passed_is_not_listed() {
    let gates = gate("a-lint", "exit 0", 30, "")
        + &needs_what_is_absent("b-test", Some("test"))
        + &gate("c-unit", "exit 0", 30, "ci_check = \"unit\"\n");

    let (ran, body) = delivered("defer-others", &gates);

    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Completed, "{}", ran.stderr());
    let results = gate_results(&events);
    assert_eq!(result_of(&results, "a-lint")["outcome"], json!("passed"));
    assert_eq!(result_of(&results, "b-test")["outcome"], json!("deferred"));
    assert_eq!(result_of(&results, "c-unit")["outcome"], json!("passed"));
    assert!(body.contains("`b-test`"), "{body}");
    assert!(
        !body.contains("a-lint") && !body.contains("c-unit"),
        "a gate that passed is listed as not run:\n{body}"
    );
}

#[test]
fn a_deferred_gate_does_not_hide_a_failing_one() {
    let gates = needs_what_is_absent("a-test", Some("test")) + &gate("b-lint", "exit 1", 30, "");

    let (ran, _) = delivered("defer-and-fail", &gates);

    let events = ran.events();
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed, "{}", ran.stderr());
    assert!(reason.contains("gate b-lint failed"), "{reason}");
    assert!(!gh_was_called(&ran));
}

#[test]
fn a_gate_that_does_not_apply_is_skipped_and_not_deferred_and_nothing_deferred_adds_nothing() {
    let gates = needs_what_is_absent("test", Some("test")).replace(
        "ci_check = \"test\"\n",
        "ci_check = \"test\"\nwhen_paths = [\"nothing-matches/**\"]\n",
    );

    let (ran, body) = delivered("defer-skipped", &gates);

    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Completed, "{}", ran.stderr());
    assert_eq!(
        result_of(&gate_results(&events), "test")["outcome"],
        json!("skipped")
    );
    assert!(
        !body.contains("Gates not run in the workspace"),
        "a body that has nothing to report says so anyway:\n{body}"
    );
}
