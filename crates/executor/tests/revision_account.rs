//! A revision that changes only what the change says about itself, and what the pull
//! request says once a revision is delivered. A review reads the commit message and the
//! pull request's description as part of the change, so a revision that corrects them is a
//! revision: it is delivered as a commit with no file changes, the description is made
//! from the revision's own account, and a revision that changed nothing at all is not
//! delivered.

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
    assert!(
        !gh_arguments(&ran).contains(&"create".to_owned()),
        "no pull request was opened"
    );
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
fn a_revision_that_only_rewrites_the_description_updates_the_pull_request_and_pushes_no_commit() {
    let (rig, head) = rig_with_branch("account-description");
    let script = format!(
        "printf '%s' '{HEAD_MESSAGE}' > .heph-commit-message; printf '## What this changes\\n\\nthe corrected account' > .heph-description.md; echo '{}'",
        json!({"agentMessage": {"text": "done"}})
    );

    let (ran, source) = run(rig, &revising(&head), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert_eq!(
        git(&source, &["rev-parse", PR_BRANCH]),
        head,
        "no commit was pushed"
    );
    assert_eq!(&gh_arguments(&ran)[..2], ["pr", "edit"]);
    assert!(gh_body(&ran).contains("the corrected account"));
}

#[test]
fn a_first_run_that_changed_nothing_and_gave_no_account_is_still_failed_whatever_its_message_says()
{
    let rig = rig("account-first", &manifest(&[]));
    let commit = rig.commit.clone();

    let (ran, _) = run(
        rig,
        &workload(&commit),
        "printf 'fix: something it did not do' > .heph-commit-message;",
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

/// The arguments of the last call to the stand-in `gh`, one per line.
fn gh_arguments(ran: &common::Ran) -> Vec<String> {
    std::fs::read_to_string(ran.scratch.path("seen/gh-args.txt"))
        .expect("gh was called")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn gh_body(ran: &common::Ran) -> String {
    std::fs::read_to_string(ran.scratch.path("seen/gh-body.txt")).expect("a body was given")
}

/// An agent that changes a file, says what it did, and says how far it got.
fn says_and_resolves(message: &str, resolution: &str) -> String {
    format!(
        "mkdir -p web; echo changed > web/changed.txt; printf '{}' > .heph-commit-message; printf '{resolution}' > .heph-resolution; echo '{}'",
        message.replace('\n', "\\n"),
        json!({"agentMessage": {"text": "done"}})
    )
}

#[test]
fn a_delivered_revision_sets_the_pull_requests_title_and_description_from_its_own_account() {
    let (rig, head) = rig_with_branch("description-files");
    let script = says_and_resolves(
        "fix: the first attempt, corrected\n\nthe cause is shown in the logs",
        "partial: item 5 is owed after the merge",
    );

    let (ran, _) = run(rig, &revising(&head), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    let arguments = gh_arguments(&ran);
    assert_eq!(&arguments[..2], ["pr", "edit"], "{arguments:?}");
    assert!(
        arguments.contains(&"52".to_owned()),
        "it is the pull request that exists: {arguments:?}"
    );
    assert!(
        arguments.contains(&"fix: the first attempt, corrected".to_owned()),
        "the title follows the new subject: {arguments:?}"
    );
    assert!(!arguments.contains(&"create".to_owned()), "{arguments:?}");
    let body = gh_body(&ran);
    assert!(body.contains("the cause is shown in the logs"), "{body}");
    assert!(
        body.contains("## What remains\n\nitem 5 is owed after the merge"),
        "{body}"
    );
    assert!(body.contains("Refs #12"), "{body}");
    assert!(!body.contains("Closes #12"), "{body}");
}

#[test]
fn a_revision_that_only_corrects_the_message_sets_the_description_too() {
    let (rig, head) = rig_with_branch("description-account");

    let (ran, _) = run(
        rig,
        &revising(&head),
        &says_only("fix: the first attempt, with its evidence\n\nthe evidence is quoted"),
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(&gh_arguments(&ran)[..2], ["pr", "edit"]);
    assert!(gh_body(&ran).contains("the evidence is quoted"));
}

#[test]
fn nothing_is_said_on_the_pull_request_when_nothing_was_delivered() {
    let (rig, head) = rig_with_branch("description-nothing");
    let (ran, _) = run(rig, &revising(&head), &says_only(HEAD_MESSAGE));
    assert_eq!(finished(&ran.events()).0, Outcome::Failed);
    assert!(!gh_was_called(&ran), "the description was not touched");

    let (rig, head) = rig_with_branch("description-refused");
    git(&rig.source, &["switch", "-q", PR_BRANCH]);
    rig.scratch
        .write("source/web/second.txt", "a person's commit\n");
    git(&rig.source, &["add", "."]);
    git(
        &rig.source,
        &["commit", "-q", "-m", "fix: a person's commit"],
    );
    git(&rig.source, &["switch", "-q", "main"]);
    let (ran, _) = run(rig, &revising(&head), &common::agent_script(""));
    assert_eq!(finished(&ran.events()).0, Outcome::Failed);
    assert!(
        !gh_was_called(&ran),
        "a push that was refused leaves the description as it was"
    );
}

#[test]
fn a_description_that_cannot_be_set_fails_the_run_and_says_so() {
    let (rig, head) = rig_with_branch("description-fails");
    let gh = stand_in_gh(&rig.scratch, 1);
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", common::agent_script("")]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let ran = run_with_env(rig.scratch, &revising(&head), None, &refs, &IDENTITY);

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(
        reason.contains("could not update the pull request"),
        "{reason}"
    );
}

#[test]
fn a_revision_that_changed_files_and_left_no_message_is_asked_once_and_then_delivered() {
    let (rig, head) = rig_with_branch("asked-for-message");
    // The first time it runs it changes a file and leaves no message; asked again, it writes one.
    let script = format!(
        "if [ -f asked ]; then printf 'fix: the first attempt, now said\\n' > .heph-commit-message; else echo more > web/more.txt; touch asked; fi; echo '{}'",
        json!({"agentMessage": {"text": "done"}})
    );

    let (ran, source) = run(rig, &revising(&head), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert_eq!(
        git(&source, &["log", "-1", "--format=%s", PR_BRANCH]),
        "fix: the first attempt, now said"
    );
}

#[test]
fn a_revision_that_still_leaves_no_message_when_asked_is_delivered_under_the_pull_requests_title() {
    let (rig, head) = rig_with_branch("never-writes-message");
    let script = format!(
        "echo more > web/more.txt; echo '{}'",
        json!({"agentMessage": {"text": "done"}})
    );

    let (ran, source) = run(rig, &revising(&head), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    let message = git(&source, &["log", "-1", "--format=%B", PR_BRANCH]);
    assert!(message.starts_with(HEAD_MESSAGE), "{message}");
    assert!(message.contains("Revised after: spec."), "{message}");
}

#[test]
fn a_revision_whose_ask_for_a_message_fails_is_still_delivered_under_the_pull_requests_title() {
    let (rig, head) = rig_with_branch("ask-fails");
    // The first run changes a file and leaves no message; when asked again, the agent fails.
    let script = format!(
        "if [ -f asked ]; then echo broke >&2; exit 1; else echo more > web/more.txt; touch asked; fi; echo '{}'",
        json!({"agentMessage": {"text": "done"}})
    );

    let (ran, source) = run(rig, &revising(&head), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    let message = git(&source, &["log", "-1", "--format=%B", PR_BRANCH]);
    assert!(message.starts_with(HEAD_MESSAGE), "{message}");
}

const DONE: &str = r#"{"agentMessage": {"text": "done"}}"#;

/// A script that counts each time the agent starts, in a file outside the workspace, then runs
/// `body`.
fn counting(rig: &Rig, body: &str) -> (String, std::path::PathBuf) {
    let calls = rig.scratch.path("calls");
    (
        format!("echo x >> {}; {body}; echo '{DONE}'", calls.display()),
        calls,
    )
}

fn starts(calls: &std::path::Path) -> usize {
    std::fs::read_to_string(calls).map_or(0, |text| text.lines().count())
}

#[test]
fn a_revision_with_no_message_is_asked_in_exactly_one_extra_agent_turn() {
    let (rig, head) = rig_with_branch("asked-once");
    let (script, calls) = counting(
        &rig,
        "if [ -f asked ]; then printf 'fix: said when asked\\n' > .heph-commit-message; else echo more > web/more.txt; touch asked; fi",
    );

    let (ran, _) = run(rig, &revising(&head), &script);

    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert_eq!(starts(&calls), 2, "one turn for the work, one for the ask");
}

#[test]
fn a_revision_that_is_asked_and_still_writes_none_is_not_asked_again() {
    let (rig, head) = rig_with_branch("asked-once-still-none");
    let (script, calls) = counting(&rig, "echo more >> web/more.txt");

    let (ran, _) = run(rig, &revising(&head), &script);

    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert_eq!(starts(&calls), 2);
}

#[test]
fn a_message_the_rule_refuses_is_asked_for_again_and_the_valid_one_is_used() {
    let (rig, head) = rig_with_branch("refused-then-valid");
    let (script, calls) = counting(
        &rig,
        "if [ -f asked ]; then printf 'fix: now conventional\\n' > .heph-commit-message; else echo more > web/more.txt; printf 'not conventional\\n' > .heph-commit-message; touch asked; fi",
    );

    let (ran, source) = run(rig, &revising(&head), &script);

    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert_eq!(starts(&calls), 2);
    assert_eq!(
        git(&source, &["log", "-1", "--format=%s", PR_BRANCH]),
        "fix: now conventional"
    );
}

#[test]
fn a_message_the_rule_refuses_twice_is_delivered_under_the_pull_requests_title() {
    let (rig, head) = rig_with_branch("refused-twice");
    let (script, calls) = counting(
        &rig,
        "echo more >> web/more.txt; printf 'not conventional\\n' > .heph-commit-message",
    );

    let (ran, source) = run(rig, &revising(&head), &script);

    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert_eq!(starts(&calls), 2);
    let message = git(&source, &["log", "-1", "--format=%B", PR_BRANCH]);
    assert!(message.starts_with(HEAD_MESSAGE), "{message}");
}

#[test]
fn the_ask_is_not_made_when_the_budget_is_already_spent() {
    let (rig, head) = rig_with_branch("budget-spent");
    let cost = json!({"cost": {"unit": "COST_UNIT_USD", "amount": 1.0, "source": "SOURCE_REPORTED_BY_RUNTIME"}});
    let (script, calls) = counting(&rig, &format!("echo more > web/more.txt; echo '{cost}'"));
    let mut value = revising(&head);
    value["limits"]["budget"]["amount"] = json!(1.0);

    let (ran, source) = run(rig, &value, &script);

    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert_eq!(starts(&calls), 1, "no turn is bought with a spent budget");
    let message = git(&source, &["log", "-1", "--format=%B", PR_BRANCH]);
    assert!(message.starts_with(HEAD_MESSAGE), "{message}");
}

#[test]
fn the_ask_gets_only_the_time_the_run_has_left() {
    let (rig, head) = rig_with_branch("time-left");
    let done = rig.scratch.path("ask-finished");
    // The work takes 1.5 of the 2 seconds allowed; the ask would take 1.5 more if it were
    // given the whole timeout, and is stopped at what is left.
    let (script, _) = counting(
        &rig,
        &format!(
            "if [ -f asked ]; then sleep 1.5; touch {}; else sleep 1.5; echo more > web/more.txt; touch asked; fi",
            done.display()
        ),
    );
    let mut value = revising(&head);
    value["limits"]["timeout"] = json!("2s");

    let (ran, source) = run(rig, &value, &script);

    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert!(!done.exists(), "the ask ran past the time the run had left");
    let message = git(&source, &["log", "-1", "--format=%B", PR_BRANCH]);
    assert!(message.starts_with(HEAD_MESSAGE), "{message}");
}
