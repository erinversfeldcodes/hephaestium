//! A revision whose only job is to bring the base branch in is delivered by the merge, with no
//! commit message from the agent, and does not fail for a race or because the branch is already
//! current.

mod common;

use std::path::PathBuf;

use common::{
    IDENTITY, Rig, agent_script, finished, gh_was_called, git, manifest, rig, run_with_env,
    stand_in_gh, workload,
};
use contracts::v1alpha1::finished::Outcome;
use serde_json::{Value, json};

const PR_BRANCH: &str = "heph/12-an-earlier-title-of-the-ticket";
const PULL: &str = "https://github.com/erinversfeldcodes/gazebolang/pull/52";

/// What an agent does when it has nothing to do: it says so and writes nothing.
fn idle() -> String {
    format!(
        "echo '{}'",
        json!({"agentMessage": {"text": "nothing to do"}})
    )
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

fn commit_on(rig: &Rig, branch: &str, path: &str, text: &str) -> String {
    git(&rig.source, &["switch", "-q", branch]);
    rig.scratch.write(&format!("source/{path}"), text);
    git(&rig.source, &["add", "."]);
    git(&rig.source, &["commit", "-q", "-m", "feat: another change"]);
    let tip = git(&rig.source, &["rev-parse", "HEAD"]);
    git(&rig.source, &["switch", "-q", "main"]);
    tip
}

/// A revision with no failing check, which exists only because the base moved.
fn bringing_in_the_base(head: &str) -> Value {
    let mut value = workload(head);
    value["revision"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": PULL,
        "branch": PR_BRANCH,
        "attempt": 1,
        "baseMoved": {"commits": ["a1b2c3d feat: another change"], "overlappingFiles": []}
    });
    value
}

fn run_revision(rig: Rig, value: &Value, script: &str) -> (common::Ran, PathBuf) {
    let gh = stand_in_gh(&rig.scratch, 0);
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let ran = run_with_env(rig.scratch, value, None, &refs, &IDENTITY);
    (ran, rig.source)
}

#[test]
fn a_revision_that_only_merges_the_base_is_delivered_without_a_commit_message() {
    let (rig, head) = rig_with_branch("only-base-merge");
    let main = commit_on(&rig, "main", "web/other.txt", "from another change\n");

    let (ran, source) = run_revision(rig, &bringing_in_the_base(&head), &idle());

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Completed, "{reason}");
    assert_eq!(
        git(&source, &["rev-parse", &format!("{PR_BRANCH}^1")]),
        head
    );
    assert_eq!(
        git(&source, &["rev-parse", &format!("{PR_BRANCH}^2")]),
        main,
        "the merge commit is what was pushed"
    );
    assert!(
        !gh_was_called(&ran),
        "the pull request's title and description are left as they are"
    );
}

#[test]
fn a_push_refused_because_the_branch_gained_commits_is_merged_and_pushed_again() {
    let (rig, head) = rig_with_branch("only-base-race");
    let gained = commit_on(&rig, PR_BRANCH, "web/second.txt", "another run's commit\n");

    let (ran, source) = run_revision(rig, &bringing_in_the_base(&head), &agent_script(""));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Completed, "{reason}");
    assert_eq!(
        git(&source, &["rev-parse", &format!("{PR_BRANCH}^2")]),
        gained,
        "what the branch gained is merged in, not forced over"
    );
    assert_eq!(
        git(&source, &["show", &format!("{PR_BRANCH}:web/changed.txt")]),
        "changed"
    );
}

#[test]
fn a_push_refused_for_commits_that_conflict_names_the_cause_and_forces_nothing() {
    let (rig, head) = rig_with_branch("only-base-race-conflict");
    let gained = commit_on(&rig, PR_BRANCH, "web/changed.txt", "someone else's\n");

    let (ran, source) = run_revision(rig, &bringing_in_the_base(&head), &agent_script(""));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("moved while the run worked"), "{reason}");
    assert_eq!(git(&source, &["rev-parse", PR_BRANCH]), gained);
}

#[test]
fn a_revision_that_finds_the_base_already_in_the_branch_is_done_and_not_failed() {
    let (rig, head) = rig_with_branch("only-base-current");

    let (ran, source) = run_revision(rig, &bringing_in_the_base(&head), &idle());

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Completed, "{reason}");
    assert!(reason.contains("nothing to bring in"), "{reason}");
    assert_eq!(git(&source, &["rev-parse", PR_BRANCH]), head);
}
