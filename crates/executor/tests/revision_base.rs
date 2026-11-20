//! A revision starts from the branch as it is and from the base branch as it is
//! now. What the base branch gained since the pull request was opened is brought
//! in with a merge commit before the agent starts, so the agent works on current
//! ground and the push is still a plain fast-forward of the branch.

mod common;

use std::path::PathBuf;

use common::{
    IDENTITY, Rig, agent_script, finished, git, manifest, rig, run_with_env, stand_in_gh, workload,
};
use contracts::v1alpha1::finished::Outcome;
use serde_json::{Value, json};

const PR_BRANCH: &str = "heph/12-an-earlier-title-of-the-ticket";

const PULL: &str = "https://github.com/erinversfeldcodes/gazebolang/pull/52";

/// A source repository where the pull request's branch has one commit past main
/// that adds `web/first.txt`, and that commit's id.
fn rig_with_branch(name: &str) -> (Rig, String) {
    rig_with_branch_manifest(name, &manifest(&[]))
}

fn rig_with_branch_manifest(name: &str, manifest_text: &str) -> (Rig, String) {
    let rig = rig(name, manifest_text);
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

/// Main gains a commit that writes `path`, after the pull request was opened.
fn main_moves(rig: &Rig, path: &str, text: &str) -> String {
    rig.scratch.write(&format!("source/{path}"), text);
    git(&rig.source, &["add", "."]);
    git(
        &rig.source,
        &["commit", "-q", "-m", "feat: something another change added"],
    );
    git(&rig.source, &["rev-parse", "main"])
}

fn revising(head: &str) -> Value {
    let mut value = workload(head);
    value["revision"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": PULL,
        "branch": PR_BRANCH,
        "attempt": 1,
        "failures": [
            {"name": "test", "result": "failure", "excerpt": "thread 'reset' panicked"}
        ]
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
fn what_the_base_gained_is_merged_in_before_the_agent_starts() {
    let (rig, head) = rig_with_branch("base-merged");
    let main = main_moves(&rig, "web/other.txt", "from another change\n");

    // The agent can only record what it saw if it saw the file.
    let script = agent_script("test -f web/other.txt && echo seen > web/saw-the-base.txt;");
    let (ran, source) = run_revision(rig, &revising(&head), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert_eq!(
        git(
            &source,
            &["show", &format!("{PR_BRANCH}:web/saw-the-base.txt")]
        ),
        "seen",
        "the agent's working tree had what main gained"
    );
    let merge = format!("{PR_BRANCH}^");
    assert_eq!(
        git(&source, &["rev-parse", &format!("{merge}^1")]),
        head,
        "the merge's first parent is the branch as it was"
    );
    assert_eq!(
        git(&source, &["rev-parse", &format!("{merge}^2")]),
        main,
        "its second parent is the base branch's tip"
    );
}

#[test]
fn a_base_that_has_not_moved_adds_no_merge() {
    let (rig, head) = rig_with_branch("base-unmoved");

    let (ran, source) = run_revision(rig, &revising(&head), &agent_script(""));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(
        git(&source, &["rev-parse", &format!("{PR_BRANCH}^")]),
        head,
        "one commit on top of the head: no merge"
    );
}

#[test]
fn files_the_base_gained_are_not_counted_as_the_agents_change() {
    // A gate that fails, and runs only for a change to web/other.txt, which is the
    // file main gained and the agent never touches.
    let text = manifest(&[])
        + "\n[gates.other]\nrun = [\"sh\", \"-c\", \"exit 1\"]\ntimeout_seconds = 30\nwhen_paths = [\"web/other.txt\"]\n";
    let (rig, head) = rig_with_branch_manifest("base-not-counted", &text);
    main_moves(&rig, "web/other.txt", "from another change\n");

    let (ran, _) = run_revision(rig, &revising(&head), &agent_script(""));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(
        outcome,
        Outcome::Completed,
        "main's file was counted as the agent's change: {reason}"
    );
}

#[test]
fn a_conflict_with_the_base_fails_the_run_and_pushes_nothing() {
    let (rig, head) = rig_with_branch("base-conflict");
    main_moves(&rig, "web/first.txt", "a different first attempt\n");

    let (ran, source) = run_revision(rig, &revising(&head), &agent_script(""));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(
        reason.contains("conflict"),
        "the reason names the conflict: {reason}"
    );
    assert_eq!(
        git(&source, &["rev-parse", PR_BRANCH]),
        head,
        "the branch is as it was"
    );
}

#[test]
fn a_first_run_given_a_base_that_has_since_moved_is_carried_onto_the_current_one() {
    let rig = rig("base-first-run", &manifest(&[]));
    let base = rig.commit.clone();
    let main = main_moves(&rig, "web/other.txt", "from another change\n");

    let (ran, source) = run_revision(rig, &workload(&base), &agent_script(""));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let pushed = git(
        &source,
        &[
            "rev-parse",
            "heph/12-make-the-reset-spec-wait-for-hydration^",
        ],
    );
    assert_eq!(
        pushed, main,
        "a first run is built on the base branch's tip, not on the commit it was given, and has no merge commit"
    );
}
