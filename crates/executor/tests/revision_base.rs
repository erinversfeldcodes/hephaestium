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

/// Resolves the conflict in `web/first.txt` the way an agent would, and only if it is there to
/// be resolved: a run that never showed the agent the markers resolves nothing.
const RESOLVE: &str = "grep -q '^<<<<<<<' web/first.txt && printf 'resolved\\n' > web/first.txt;";

fn conflicting_rig(name: &str, manifest_text: &str) -> (Rig, String, String) {
    let (rig, head) = rig_with_branch_manifest(name, manifest_text);
    let main = main_moves(&rig, "web/first.txt", "a different first attempt\n");
    (rig, head, main)
}

#[test]
fn a_conflict_with_the_base_reaches_the_agent_and_a_resolved_tree_is_delivered() {
    let (rig, head, main) = conflicting_rig("base-conflict-resolved", &manifest(&[]));

    let script = agent_script(RESOLVE);
    let (ran, source) = run_revision(rig, &revising(&head), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Completed, "{reason}");
    assert_eq!(
        git(&source, &["show", &format!("{PR_BRANCH}:web/first.txt")]),
        "resolved",
        "what the agent resolved is what was pushed"
    );
    let merge = format!("{PR_BRANCH}^");
    assert_eq!(git(&source, &["rev-parse", &format!("{merge}^1")]), head);
    assert_eq!(
        git(&source, &["rev-parse", &format!("{merge}^2")]),
        main,
        "the resolution is a merge of the base's tip into the branch"
    );
}

#[test]
fn the_gates_judge_the_tree_the_agent_resolved_and_not_the_conflicted_one() {
    // Passes only once the agent has reconfirmed the file: the gate runs after it.
    let text = manifest(&[])
        + "\n[gates.resolved]\nrun = [\"sh\", \"-c\", \"grep -q resolved web/first.txt\"]\ntimeout_seconds = 30\nwhen_paths = [\"web/first.txt\"]\n";
    let (rig, head, _) = conflicting_rig("base-conflict-gates", &text);

    let (ran, _) = run_revision(rig, &revising(&head), &agent_script(RESOLVE));

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Completed, "{reason}");
}

#[test]
fn a_conflict_marker_the_agent_leaves_is_never_committed() {
    let (rig, head, _) = conflicting_rig("base-conflict-left", &manifest(&[]));

    let (ran, source) = run_revision(rig, &revising(&head), &agent_script(""));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("web/first.txt"), "names the file: {reason}");
    assert!(reason.contains("conflict"), "says why: {reason}");
    assert_eq!(
        git(&source, &["rev-parse", PR_BRANCH]),
        head,
        "nothing was pushed: the branch is as it was"
    );
}

#[test]
fn deleting_the_conflicting_file_is_a_resolution() {
    let (rig, head, _) = conflicting_rig("base-conflict-deleted", &manifest(&[]));

    let (ran, source) = run_revision(rig, &revising(&head), &agent_script("rm web/first.txt;"));

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Completed, "{reason}");
    assert_eq!(
        git(
            &source,
            &["ls-tree", "--name-only", PR_BRANCH, "web/first.txt"]
        ),
        "",
        "the file is gone from what was pushed"
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
