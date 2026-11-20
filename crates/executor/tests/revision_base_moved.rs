//! A revision can exist because the base branch moved and not because a check failed.
//! It says what came in, so the agent can reconfirm that the ticket's problem is still
//! there and that its change still does what it set out to, with no failing check to
//! point at.

mod common;

use std::path::PathBuf;

use common::{
    IDENTITY, Rig, agent_script, finished, git, manifest, rig, run_with_env, stand_in_gh, workload,
};
use contracts::v1alpha1::Workload;
use contracts::v1alpha1::finished::Outcome;
use executor::adapter::Adapter;
use executor::claude_code::ClaudeCode;
use serde_json::{Value, json};

const PR_BRANCH: &str = "heph/12-an-earlier-title-of-the-ticket";
const PULL: &str = "https://github.com/erinversfeldcodes/gazebolang/pull/52";

fn base_moved() -> Value {
    json!({
        "commits": [
            "a1b2c3d fix: stop the reset spec racing hydration",
            "e4f5a6b chore: bump the toolchain"
        ],
        "overlappingFiles": ["e2e/tests/account.spec.ts"]
    })
}

/// A revision of the pull request with the given failures and what the base gained.
fn revising(head: &str, failures: Value, base_moved: Option<Value>) -> Value {
    let mut value = workload(head);
    value["revision"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": PULL,
        "branch": PR_BRANCH,
        "attempt": 1,
        "failures": failures,
    });
    if let Some(moved) = base_moved {
        value["revision"]["baseMoved"] = moved;
    }
    value
}

fn one_failure() -> Value {
    json!([{"name": "test", "result": "failure", "excerpt": "thread 'reset' panicked"}])
}

fn prompt_of(value: &Value) -> String {
    let workload: Workload = serde_json::from_value(value.clone()).expect("a workload");
    let plan = ClaudeCode.plan(&workload, std::path::Path::new("/work/w"));
    let at = plan
        .command
        .iter()
        .position(|word| word == "-p")
        .expect("-p");
    plan.command[at + 1].clone()
}

fn rig_with_branch(name: &str) -> (Rig, String) {
    let rig = rig(name, &manifest(&[]));
    git(&rig.source, &["switch", "-q", "-c", PR_BRANCH]);
    rig.scratch
        .write("source/web/first.txt", "the first attempt\n");
    git(&rig.source, &["add", "."]);
    git(
        &rig.source,
        &["commit", "-q", "-m", "fix: the first attempt"],
    );
    let head = git(&rig.source, &["rev-parse", "HEAD"]);
    git(&rig.source, &["switch", "-q", "main"]);
    (rig, head)
}

fn run_revision(rig: Rig, value: &Value) -> (common::Ran, PathBuf) {
    let gh = stand_in_gh(&rig.scratch, 0);
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", agent_script("")]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let ran = run_with_env(rig.scratch, value, None, &refs, &IDENTITY);
    (ran, rig.source)
}

#[test]
fn the_prompt_of_a_revision_for_a_moved_base_names_what_came_in() {
    let head = "0123456789abcdef0123456789abcdef01234567";
    let prompt = prompt_of(&revising(head, json!([]), Some(base_moved())));

    for expected in [
        "a1b2c3d fix: stop the reset spec racing hydration",
        "e4f5a6b chore: bump the toolchain",
        "e2e/tests/account.spec.ts",
    ] {
        assert!(prompt.contains(expected), "{expected} is missing: {prompt}");
    }
}

#[test]
fn it_asks_the_agent_to_reconfirm_the_problem_and_its_change() {
    let head = "0123456789abcdef0123456789abcdef01234567";
    let prompt = prompt_of(&revising(head, json!([]), Some(base_moved())));

    assert!(
        prompt.contains("still exists") || prompt.contains("still there"),
        "it asks whether the ticket's problem is still there: {prompt}"
    );
    assert!(
        prompt.contains("still does what"),
        "it asks whether the change still does what it set out to: {prompt}"
    );
}

#[test]
fn it_does_not_say_a_check_failed_when_none_did() {
    let head = "0123456789abcdef0123456789abcdef01234567";
    let prompt = prompt_of(&revising(head, json!([]), Some(base_moved())));

    assert!(!prompt.contains("checks failed"), "{prompt}");
    assert!(!prompt.contains("failing checks"), "{prompt}");
}

#[test]
fn a_revision_with_both_tells_the_agent_both() {
    let head = "0123456789abcdef0123456789abcdef01234567";
    let prompt = prompt_of(&revising(head, one_failure(), Some(base_moved())));

    assert!(prompt.contains("thread 'reset' panicked"), "{prompt}");
    assert!(prompt.contains("a1b2c3d"), "{prompt}");
}

#[test]
fn a_revision_for_a_moved_base_is_accepted_with_no_failing_check() {
    let (rig, head) = rig_with_branch("moved-accepted");

    let (ran, _) = run_revision(rig, &revising(&head, json!([]), Some(base_moved())));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
}

#[test]
fn a_revision_with_neither_a_failure_nor_a_moved_base_is_still_refused() {
    let (rig, head) = rig_with_branch("moved-neither");

    let (ran, _) = run_revision(rig, &revising(&head, json!([]), None));

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert_eq!(ran.output.stdout, b"", "events were written");
}

#[test]
fn a_moved_base_that_names_no_commit_is_refused() {
    let (rig, head) = rig_with_branch("moved-no-commit");

    let (ran, _) = run_revision(
        rig,
        &revising(&head, json!([]), Some(json!({"commits": []}))),
    );

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert_eq!(ran.output.stdout, b"", "events were written");
}
