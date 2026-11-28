//! Every file an agent is told to write is named by its absolute path, because the agent's
//! shell keeps its directory between calls and a bare name lands wherever it last changed to.

mod common;

use std::path::Path;

use common::{IDENTITY, finished, git, manifest, rig, run_with_env, stand_in_gh, workload};
use contracts::v1alpha1::Workload;
use contracts::v1alpha1::finished::Outcome;
use executor::adapter::Adapter;
use executor::claude_code::ClaudeCode;
use serde_json::{Value, json};

const WORKSPACE: &str = "/work/w";
const BRANCH: &str = "heph/12-the-change";
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const LENS_TEXT: &str = "---\nname: rust\n---\nLook at ownership and error handling.";
const FILES: [&str; 4] = [
    ".heph-commit-message",
    ".heph-description.md",
    ".heph-resolution",
    ".heph-question",
];

fn prompt_of(value: &Value) -> String {
    let workload: Workload = serde_json::from_value(value.clone()).expect("a workload");
    ClaudeCode.plan(&workload, Path::new(WORKSPACE)).stdin
}

fn revision(pull: &str, failures: &Value, moved: &Value) -> Value {
    let mut value = workload(COMMIT);
    value["revision"] = json!({
        "pullRequestNumber": pull,
        "pullRequestUrl": "",
        "branch": "",
        "attempt": 1,
        "failures": failures,
        "baseMoved": moved,
    });
    value
}

/// Every `.heph-` in the text that is not directly after the workspace's own path.
fn bare_names(prompt: &str) -> Vec<String> {
    prompt
        .match_indices(".heph-")
        .filter(|(at, _)| !prompt[..*at].ends_with(&format!("{WORKSPACE}/")))
        .map(|(at, _)| prompt[at.saturating_sub(20)..(at + 24).min(prompt.len())].to_owned())
        .collect()
}

#[test]
fn no_kind_of_prompt_names_a_file_by_a_bare_name() {
    let failure = json!([{"name": "gate", "result": "failed", "excerpt": "boom"}]);
    let moved = json!({"commits": ["a1b2c3d feat: x"], "overlappingFiles": ["a.txt"], "conflictedFiles": ["a.txt"]});
    let mut review = workload(COMMIT);
    review["review"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": "https://github.com/o/r/pull/52",
        "lens": "rust",
        "lensDocument": LENS_TEXT,
    });
    let mut completeness = workload(COMMIT);
    completeness["completeness"] = json!({ "skillDocument": "the skill" });
    let mut gap = workload(COMMIT);
    gap["gap"] = json!({ "skillDocument": "the skill", "because": "not met" });

    let kinds = [
        ("first run", workload(COMMIT)),
        ("revision", revision("7", &failure, &moved)),
        ("carry", revision("0", &json!([]), &moved)),
        ("pre-push", revision("0", &failure, &moved)),
        ("review", review),
        ("completeness", completeness),
        ("gap", gap),
    ];
    for (kind, value) in kinds {
        let prompt = prompt_of(&value);
        let bare = bare_names(&prompt);
        assert!(bare.is_empty(), "{kind} prompt has bare names: {bare:?}");
    }
}

#[test]
fn the_first_run_prompt_holds_each_file_by_absolute_path() {
    let prompt = prompt_of(&workload(COMMIT));
    for name in FILES {
        assert!(prompt.contains(&format!("{WORKSPACE}/{name}")), "{name}");
    }
}

#[test]
fn the_review_prompt_holds_the_verdict_by_absolute_path() {
    let mut value = workload(COMMIT);
    value["review"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": "https://github.com/o/r/pull/52",
        "lens": "rust",
        "lensDocument": LENS_TEXT,
    });
    let prompt = prompt_of(&value);
    assert!(
        prompt.contains(&format!("`{WORKSPACE}/.heph-verdict.json`")),
        "{prompt}"
    );
}

#[test]
fn the_completeness_and_gap_prompts_hold_their_files_by_absolute_path() {
    let mut value = workload(COMMIT);
    value["completeness"] = json!({ "skillDocument": "the skill" });
    assert!(prompt_of(&value).contains(&format!("`{WORKSPACE}/.heph-completeness.json`")));
    let mut value = workload(COMMIT);
    value["gap"] = json!({ "skillDocument": "the skill", "because": "not met" });
    assert!(prompt_of(&value).contains(&format!("`{WORKSPACE}/.heph-gap.md`")));
}

#[test]
fn a_verdict_written_after_changing_directory_is_still_delivered() {
    let rig = rig("review-cd", &manifest(&[]));
    git(&rig.source, &["switch", "-q", "-c", BRANCH]);
    rig.scratch.write("source/web/change.txt", "the change\n");
    git(&rig.source, &["add", "."]);
    git(&rig.source, &["commit", "-q", "-m", "feat: the change"]);
    let head = git(&rig.source, &["rev-parse", "HEAD"]);
    git(&rig.source, &["switch", "-q", "main"]);
    let mut value = workload(&head);
    value["review"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": "https://github.com/o/r/pull/52",
        "lens": "rust",
        "lensDocument": LENS_TEXT,
    });
    let said = json!({
        "schema_version": 0, "lens": "rust", "commit": head, "verdict": "approved",
        "findings": [{"severity": "P3", "blocking": false, "title": "A name", "evidence": "web/change.txt:1"}],
        "notes": ["Small."]
    });
    // The agent starts at the workspace root, goes into a subdirectory, and writes by the
    // absolute path it was given.
    let script = format!(
        "root=$(pwd); cd web; printf '%s' '{said}' > \"$root/.heph-verdict.json\"; echo '{}'",
        json!({"agentMessage": {"text": "reviewed"}})
    );
    let gh = stand_in_gh(&rig.scratch, 0);
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let ran = run_with_env(rig.scratch, &value, None, &refs, &IDENTITY);
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
}

#[test]
fn a_verdict_left_in_a_subdirectory_is_not_read_and_the_failure_says_where_it_was() {
    let rig = rig("review-misplaced", &manifest(&[]));
    git(&rig.source, &["switch", "-q", "-c", BRANCH]);
    rig.scratch.write("source/web/change.txt", "the change\n");
    git(&rig.source, &["add", "."]);
    git(&rig.source, &["commit", "-q", "-m", "feat: the change"]);
    let head = git(&rig.source, &["rev-parse", "HEAD"]);
    git(&rig.source, &["switch", "-q", "main"]);
    let mut value = workload(&head);
    value["review"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": "https://github.com/o/r/pull/52",
        "lens": "rust",
        "lensDocument": LENS_TEXT,
    });
    let said = json!({
        "schema_version": 0, "lens": "rust", "commit": head, "verdict": "approved",
        "findings": [], "notes": ["Small."]
    });
    let script = format!(
        "cd web; printf '%s' '{said}' > .heph-verdict.json; echo '{}'",
        json!({"agentMessage": {"text": "reviewed"}})
    );
    let gh = stand_in_gh(&rig.scratch, 0);
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let ran = run_with_env(rig.scratch, &value, None, &refs, &IDENTITY);
    let events = ran.events();
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(reason.contains("left no verdict"), "{reason}");
    assert!(reason.contains("web/.heph-verdict.json"), "{reason}");
    assert!(reason.contains("not read"), "{reason}");
}
