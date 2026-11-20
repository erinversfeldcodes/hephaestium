//! A revision that changes only what the change says about itself. A review reads the
//! commit message as part of the change, so a revision that corrects it is a revision:
//! it is delivered as a commit with no file changes, and a revision that changed
//! nothing at all still is not.

mod common;

use common::{
    IDENTITY, Rig, finished, gh_was_called, git, manifest, rig, run_with_env, stand_in_gh, workload,
};
use contracts::v1alpha1::finished::Outcome;
use serde_json::{Value, json};

const PR_BRANCH: &str = "heph/12-an-earlier-title-of-the-ticket";
const PULL: &str = "https://github.com/erinversfeldcodes/gazebolang/pull/52";
const HEAD_MESSAGE: &str = "fix: the first attempt";

fn rig_with_branch(name: &str) -> (Rig, String) {
    let rig = rig(name, &manifest(&[]));
    git(&rig.source, &["switch", "-q", "-c", PR_BRANCH]);
    rig.scratch
        .write("source/web/first.txt", "the first attempt\n");
    git(&rig.source, &["add", "."]);
    git(&rig.source, &["commit", "-q", "-m", HEAD_MESSAGE]);
    let head = git(&rig.source, &["rev-parse", "HEAD"]);
    git(&rig.source, &["switch", "-q", "main"]);
    (rig, head)
}

fn revising(head: &str) -> Value {
    let mut value = workload(head);
    value["revision"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": PULL,
        "branch": PR_BRANCH,
        "attempt": 1,
        "failures": [{"name": "spec", "result": "failure", "excerpt": "item 4 not done"}]
    });
    value
}

/// An agent that writes a commit message and changes no file.
fn says_only(message: &str) -> String {
    format!(
        "printf '{}' > .heph-commit-message; echo '{}'",
        message.replace('\n', "\\n"),
        json!({"agentMessage": {"text": "done"}})
    )
}

fn run(rig: Rig, value: &Value, script: &str) -> (common::Ran, std::path::PathBuf) {
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
fn a_revision_that_only_corrects_the_message_is_delivered_as_a_commit_with_no_file_changes() {
    let (rig, head) = rig_with_branch("account-new");
    let script = says_only("fix: the first attempt, with its evidence\n\nthe cause is shown");

    let (ran, source) = run(rig, &revising(&head), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert_eq!(
        git(&source, &["rev-parse", &format!("{PR_BRANCH}^")]),
        head,
        "one commit on top of the branch's head"
    );
    assert_eq!(
        git(&source, &["diff", "--stat", &head, PR_BRANCH]),
        "",
        "the tree is the one the branch had"
    );
    assert!(
        git(&source, &["log", "-1", "--format=%B", PR_BRANCH]).contains("the cause is shown"),
        "the new account is the commit's message"
    );
    assert!(!gh_was_called(&ran), "no pull request was opened");
}

#[test]
fn a_revision_that_changed_nothing_and_says_what_the_head_already_says_is_still_failed() {
    let (rig, head) = rig_with_branch("account-same");

    let (ran, source) = run(rig, &revising(&head), &says_only(HEAD_MESSAGE));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("nothing to deliver"), "{reason}");
    assert_eq!(
        git(&source, &["rev-parse", PR_BRANCH]),
        head,
        "nothing was pushed"
    );
}

#[test]
fn a_first_run_that_changed_nothing_is_still_failed_whatever_it_says() {
    let rig = rig("account-first", &manifest(&[]));
    let commit = rig.commit.clone();

    let (ran, _) = run(
        rig,
        &workload(&commit),
        &says_only("fix: something it did not do"),
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("nothing to deliver"), "{reason}");
}

#[test]
fn a_revision_that_changed_files_is_delivered_as_it_always_was() {
    let (rig, head) = rig_with_branch("account-files");
    let script = common::agent_script("");

    let (ran, source) = run(rig, &revising(&head), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert!(
        git(&source, &["diff", "--stat", &head, PR_BRANCH]).contains("changed.txt"),
        "the file change is pushed"
    );
}
