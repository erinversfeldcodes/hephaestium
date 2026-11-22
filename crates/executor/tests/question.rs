//! A run that cannot go on, or finds nothing to change, ends blocked with what a person
//! needs to know: the agent's question, or its account, or the proposal that the ticket be
//! closed because the base already fixes it. It is not a failure, and nothing is pushed.

mod common;

use common::{IDENTITY, agent_script, gh_was_called, git, manifest, rig, run};
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use serde_json::json;

const ACCOUNT: &str = "I could not reproduce the panic: the handler returns before the lock is taken. The next place to look is the preview application log.";

/// What the run said it ended in: the outcome, the reason and the question.
fn ended(ran: &common::Ran) -> (Outcome, String, String) {
    match ran.events().last().and_then(|e| e.kind.as_ref()) {
        Some(Kind::Finished(done)) => (
            Outcome::try_from(done.outcome).expect("a known outcome"),
            done.reason.clone(),
            done.question.clone(),
        ),
        other => panic!("the stream does not end with finished: {other:?}"),
    }
}

fn message(text: &str) -> String {
    format!(" echo '{}'", json!({"agentMessage": {"text": text}}))
}

fn delivering(name: &str, script: &str) -> (common::Ran, std::path::PathBuf, String) {
    run(rig(name, &manifest(&[])), script, true, 0, &IDENTITY)
}

fn nothing_left_the_machine(ran: &common::Ran, source: &std::path::Path) {
    assert!(!gh_was_called(ran), "gh was called");
    assert!(
        !ran.events()
            .iter()
            .any(|e| matches!(e.kind, Some(Kind::PullRequestOpened(_)))),
        "a pull request was opened"
    );
    assert!(
        git(source, &["branch", "--list", "heph/*"]).is_empty(),
        "a branch was pushed"
    );
}

#[test]
fn a_question_the_agent_leaves_ends_the_run_blocked_and_keeps_the_workspace() {
    let question = "Once or until it expires? I would choose once, because the ticket says the token is single use.";
    let script = format!(
        "mkdir -p web; echo partial > web/partial.txt; printf '{question}' > .heph-question;{}",
        message("I stopped to ask.")
    );

    let (ran, source, _) = delivering("question-file", &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason, asked) = ended(&ran);
    assert_eq!(outcome, Outcome::Blocked, "{reason}");
    assert_eq!(asked, question, "the question is in the event");
    let record = ran.record();
    assert_eq!(record.question, question, "and in the record");
    assert_eq!(record.outcome, Outcome::Blocked as i32);
    let workspace = ran.scratch.path("work").join(&record.workload_id);
    assert!(
        workspace.join("web/partial.txt").exists(),
        "the workspace is kept as the agent left it"
    );
    nothing_left_the_machine(&ran, &source);
}

#[test]
fn a_first_run_that_changes_nothing_and_leaves_an_account_ends_blocked_with_the_account() {
    let (ran, source, _) = delivering("question-account", message(ACCOUNT).trim_start());

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason, asked) = ended(&ran);
    assert_eq!(outcome, Outcome::Blocked, "{reason}");
    assert!(asked.contains(ACCOUNT), "{asked}");
    assert_eq!(ran.record().question, asked);
    nothing_left_the_machine(&ran, &source);
}

#[test]
fn a_first_run_that_changes_nothing_and_leaves_no_account_still_fails() {
    let (ran, source, _) = delivering(
        "question-no-account",
        "printf 'fix: nothing\\n' > .heph-commit-message;",
    );

    let (outcome, reason, asked) = ended(&ran);
    assert_eq!(outcome, Outcome::Failed, "{}", ran.stderr());
    assert!(reason.contains("nothing to deliver"), "{reason}");
    assert_eq!(asked, "", "a failure asks nothing");
    nothing_left_the_machine(&ran, &source);
}

#[test]
fn an_account_that_is_only_blank_is_no_account() {
    let (ran, _, _) = delivering(
        "question-blank",
        &format!(
            "printf 'fix: nothing\\n' > .heph-commit-message;{}",
            message("   ")
        ),
    );

    assert_eq!(ended(&ran).0, Outcome::Failed, "{}", ran.stderr());
}

#[test]
fn a_ticket_the_base_already_fixes_ends_blocked_proposing_a_person_close_it() {
    let script = format!(
        "printf 'resolved-by: 0123abc\\n' > .heph-resolution;{}",
        message("The change is already on main.")
    );

    let (ran, source, _) = delivering("question-resolved", &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason, asked) = ended(&ran);
    assert_eq!(outcome, Outcome::Blocked, "{reason}");
    assert!(asked.contains("0123abc"), "it names the commit: {asked}");
    assert!(asked.contains("close"), "it proposes closing: {asked}");
    assert!(
        asked.contains("The change is already on main."),
        "it carries the agent's account: {asked}"
    );
    nothing_left_the_machine(&ran, &source);
}

#[test]
fn a_resolution_that_names_no_commit_is_not_a_claim_the_base_fixes_the_ticket() {
    let script = format!(
        "printf 'resolved-by: the other branch\\n' > .heph-resolution; printf 'fix: nothing\\n' > .heph-commit-message;{}",
        message("   ")
    );

    let (ran, _, _) = delivering("question-resolved-no-commit", &script);

    assert_eq!(ended(&ran).0, Outcome::Failed, "{}", ran.stderr());
}

#[test]
fn a_run_that_changed_files_and_asked_nothing_is_delivered_as_it_always_was() {
    let (ran, _, _) = delivering("question-delivered", &agent_script(""));

    assert_eq!(ended(&ran).0, Outcome::Completed, "{}", ran.stderr());
    assert_eq!(ended(&ran).2, "");
}
