//! A revision that changes only the pull request's description. It is delivered to the pull
//! request and not to its branch, only when the description differs from the one the pull
//! request has, and it says so when it is over.

mod common;

use std::fmt::Write as _;
use std::path::Path;

use common::{IDENTITY, Rig, finished, git, manifest, rig, run_with_env, stand_in_gh, workload};
use contracts::v1alpha1::finished::Outcome;
use serde_json::{Value, json};

const PR_BRANCH: &str = "heph/12-an-earlier-title-of-the-ticket";
const PULL: &str = "https://github.com/erinversfeldcodes/gazebolang/pull/52";
const HEAD_MESSAGE: &str = "fix: the first attempt";
const ACCOUNT: &str = "## What this changes\\n\\nthe corrected account";
const LENS: &str = "---\nname: rust\napplies: always\n---\nLook at ownership.";

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

fn say() -> String {
    format!("echo '{}'", json!({"agentMessage": {"text": "done"}}))
}

/// An agent that writes the head's own message and a description, and changes no file.
fn rewrites_description(text: &str) -> String {
    format!(
        "printf '%s' '{HEAD_MESSAGE}' > .heph-commit-message; printf '{text}' > .heph-description.md; {}",
        say()
    )
}

fn run_with_gh(rig: Rig, value: &Value, script: &str, gh: &Path) -> common::Ran {
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    run_with_env(rig.scratch, value, None, &refs, &IDENTITY)
}

/// A `gh` that serves `body` for `pr view`, and keeps the body it is given by `pr edit`.
fn gh_serving(rig: &Rig, body: &str) -> std::path::PathBuf {
    let seen = rig.scratch.path("seen");
    std::fs::create_dir_all(&seen).expect("a directory");
    rig.scratch.write("served-body.md", body);
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > {seen}/gh-args.txt\nif [ \"$1 $2\" = 'pr view' ]; then cat {body}; exit 0; fi\nprev=''\nfor a in \"$@\"; do if [ \"$prev\" = '--body-file' ]; then cp \"$a\" {seen}/gh-body.txt; fi; prev=\"$a\"; done\necho {PULL}\n",
        seen = seen.display(),
        body = rig.scratch.path("served-body.md").display(),
    );
    let gh = rig.scratch.path("serving-gh");
    std::fs::write(&gh, script).expect("a script");
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("to chmod");
    gh
}

fn gh_edited(ran: &common::Ran) -> bool {
    ran.scratch.path("seen/gh-body.txt").exists()
}

#[test]
fn a_revision_that_rewrites_the_current_description_word_for_word_fails() {
    // First, what delivering this description would set: the pull request then has exactly it.
    let (rig, head) = rig_with_branch("description-same-first");
    let gh = stand_in_gh(&rig.scratch, 0);
    let ran = run_with_gh(rig, &revising(&head), &rewrites_description(ACCOUNT), &gh);
    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    let current = std::fs::read_to_string(ran.scratch.path("seen/gh-body.txt")).expect("a body");

    let (rig, head) = rig_with_branch("description-same-second");
    let gh = gh_serving(&rig, &current);
    let ran = run_with_gh(rig, &revising(&head), &rewrites_description(ACCOUNT), &gh);

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(reason.contains("nothing to deliver"), "{reason}");
    assert!(!gh_edited(&ran), "the pull request was not edited");
}

#[test]
fn a_revision_whose_description_differs_from_the_current_one_updates_it_and_says_nothing_was_pushed()
 {
    let (rig, head) = rig_with_branch("description-differs");
    let gh = gh_serving(&rig, "an older account\n");
    let ran = run_with_gh(rig, &revising(&head), &rewrites_description(ACCOUNT), &gh);

    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    assert!(gh_edited(&ran));
    let record = ran.record();
    assert!(
        record.pull_request.expect("a pull request").nothing_pushed,
        "the record says no commit was pushed"
    );
}

/// An agent that counts its starts and does the `n`th of these on the `n`th.
fn agent(root: &Path, starts: &[String]) -> String {
    let mut text = format!(
        "mkdir -p {root}; n=$(cat {root}/n 2>/dev/null || echo 0); n=$((n+1)); echo $n > {root}/n\ncase $n in\n",
        root = root.display()
    );
    for (at, start) in starts.iter().enumerate() {
        let _ = writeln!(text, "{}) {start} ;;", at + 1);
    }
    text.push_str("*) echo unexpected start >&2; exit 9 ;;\nesac\n");
    text
}

fn reads(kind: &str, blocking: bool) -> String {
    let findings = if blocking {
        json!([{
            "severity": "P1",
            "blocking": true,
            "title": "the description claims a test it does not name",
            "evidence": "the description shows it.",
            "revision": "Name the test."
        }])
    } else {
        json!([])
    };
    let verdict = json!({
        "schema_version": 0,
        "lens": "rust",
        "commit": "COMMIT",
        "verdict": kind,
        "findings": findings,
    });
    format!(
        "c=$(git rev-parse HEAD); printf '%s' '{verdict}' | sed \"s/COMMIT/$c/\" > .heph-verdict.json; {}",
        say()
    )
}

#[test]
fn a_lens_fix_that_changes_only_the_description_commits_and_pushes_nothing() {
    let (rig, head) = rig_with_branch("description-lens-fix");
    let root = rig.scratch.path("agent");
    let starts = [
        rewrites_description(ACCOUNT),
        reads("needs_revision", true),
        format!(
            "printf '## What this changes\\n\\nthe corrected account, naming its test' > .heph-description.md; {}",
            say()
        ),
        reads("approved", false),
    ];
    let script = agent(&root, &starts);
    let mut value = revising(&head);
    value["lenses"] = json!([{"pack": "rust", "document": LENS}]);
    value["prepushRounds"] = json!(2);
    let gh = stand_in_gh(&rig.scratch, 0);
    let source = rig.source.clone();

    let ran = run_with_gh(rig, &value, &script, &gh);

    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    assert_eq!(
        git(&source, &["rev-parse", PR_BRANCH]),
        head,
        "no commit, empty or not, was pushed to the branch"
    );
    let body = std::fs::read_to_string(ran.scratch.path("seen/gh-body.txt")).expect("a body");
    assert!(body.contains("naming its test"), "{body}");
}
