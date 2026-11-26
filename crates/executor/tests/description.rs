//! The pull request's description says what the ticket asked for, what the change
//! does, what remains and what the risks are, in that shape. The agent writes it to
//! `.heph-description.md`, apart from the commit message: the commit stays short, and
//! the description is the pull request's own account. What remains is not left to the
//! agent: it is what the agent said in `.heph-resolution`, so it always agrees with
//! whether the pull request closes the ticket or only refers to it.

mod common;

use std::fmt::Write as _;

use common::{
    BRANCH, IDENTITY, Rig, agent_script, finished, git, manifest, rig, run_with_env, stand_in_gh,
    workload,
};
use contracts::v1alpha1::Workload;
use contracts::v1alpha1::finished::Outcome;
use executor::adapter::Adapter;
use executor::claude_code::ClaudeCode;
use serde_json::{Value, json};

const DESCRIPTION: &str = "## What the ticket asked for\\n\\nThe reset spec must wait for hydration.\\n\\n## What this changes\\n\\n- The spec waits for the page to be interactive. Shown by the spec passing 20 times.\\n\\n## Risks and shortcomings\\n\\n- Only the reset spec was changed.\\n";

/// A delivered pull request whose agent also left this description and resolution.
fn delivered(
    name: &str,
    description: Option<&str>,
    resolution: Option<&str>,
) -> (common::Ran, Rig, String) {
    let rig = rig(name, &manifest(&[]));
    let mut extra = String::new();
    if let Some(text) = description {
        let _ = write!(extra, "printf '{text}' > .heph-description.md;");
    }
    if let Some(text) = resolution {
        let _ = write!(extra, "printf '{text}' > .heph-resolution;");
    }
    let gh = stand_in_gh(&rig.scratch, 0);
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", agent_script(&extra)]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let source = rig.source.clone();
    let commit = rig.commit.clone();
    let ran = run_with_env(rig.scratch, &workload(&commit), None, &refs, &IDENTITY);
    let body = std::fs::read_to_string(ran.scratch.path("seen/gh-body.txt")).unwrap_or_default();
    (
        ran,
        Rig {
            scratch: common::Scratch::new(&format!("{name}-after")),
            commit,
            source,
        },
        body,
    )
}

#[test]
fn the_description_the_agent_wrote_is_the_pull_requests_body() {
    let (ran, _, body) = delivered("description-used", Some(DESCRIPTION), Some("complete\\n"));

    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    for heading in [
        "## What the ticket asked for",
        "## What this changes",
        "## Risks and shortcomings",
    ] {
        assert!(body.contains(heading), "{heading} is missing: {body}");
    }
    assert!(
        body.contains("Shown by the spec passing 20 times"),
        "{body}"
    );
}

#[test]
fn the_commit_message_body_is_not_repeated_in_the_description() {
    let (_, _, body) = delivered(
        "description-not-repeated",
        Some(DESCRIPTION),
        Some("complete\\n"),
    );

    assert!(
        !body.contains("The spec clicked before the page was interactive"),
        "the commit message's body is the commit's: {body}"
    );
}

#[test]
fn without_a_description_the_commit_messages_body_is_used_as_it_was() {
    let (_, _, body) = delivered("description-absent", None, Some("complete\\n"));

    assert!(
        body.contains("The spec clicked before the page was interactive"),
        "{body}"
    );
    assert!(!body.contains("## What remains"), "{body}");
}

#[test]
fn a_partial_resolution_says_what_remains_under_its_own_heading() {
    let (_, _, body) = delivered(
        "description-remains",
        Some(DESCRIPTION),
        Some("partial: the cold run is owed after the merge\\n"),
    );

    assert!(
        body.contains("## What remains\n\nthe cold run is owed after the merge"),
        "{body}"
    );
    assert!(!body.contains("Not resolved:"), "{body}");
    assert_eq!(body.trim_end().lines().last(), Some("Refs #12"), "{body}");
}

#[test]
fn a_complete_resolution_has_no_section_for_what_remains() {
    let (_, _, body) = delivered(
        "description-complete",
        Some(DESCRIPTION),
        Some("complete\\n"),
    );

    assert!(!body.contains("## What remains"), "{body}");
    assert_eq!(body.trim_end().lines().last(), Some("Closes #12"), "{body}");
}

#[test]
fn the_description_file_is_never_part_of_the_change() {
    let rig = rig("description-not-committed", &manifest(&[]));
    let source = rig.source.clone();
    let gh = stand_in_gh(&rig.scratch, 0);
    let script = agent_script(&format!("printf '{DESCRIPTION}' > .heph-description.md;"));
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let commit = rig.commit.clone();

    let ran = run_with_env(rig.scratch, &workload(&commit), None, &refs, &IDENTITY);

    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    let files = git(&source, &["ls-tree", "-r", "--name-only", BRANCH]);
    assert!(!files.contains(".heph-description.md"), "{files}");
}

fn prompt_of(value: &Value) -> String {
    let workload: Workload = serde_json::from_value(value.clone()).expect("a workload");
    let plan = ClaudeCode.plan(&workload, std::path::Path::new("/work/w"));
    plan.stdin
}

#[test]
fn the_prompt_tells_the_agent_the_description_and_its_sections() {
    let prompt = prompt_of(&workload("0123456789abcdef0123456789abcdef01234567"));

    assert!(prompt.contains(".heph-description.md"), "{prompt}");
    for section in [
        "What the ticket asked for",
        "What this changes",
        "Risks and shortcomings",
    ] {
        assert!(prompt.contains(section), "{section} is missing: {prompt}");
    }
    assert!(
        prompt.contains("What remains") && prompt.contains(".heph-resolution"),
        "it says what remains comes from the resolution: {prompt}"
    );
}

#[test]
fn a_revisions_prompt_says_the_description_is_rewritten_from_the_file() {
    let mut value = workload("0123456789abcdef0123456789abcdef01234567");
    value["revision"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": "https://github.com/erinversfeldcodes/gazebolang/pull/52",
        "branch": "heph/12-an-earlier-title-of-the-ticket",
        "attempt": 1,
        "failures": [{"name": "test", "result": "failure", "excerpt": "x"}]
    });

    let prompt = prompt_of(&value);

    assert!(prompt.contains(".heph-description.md"), "{prompt}");
    assert!(prompt.contains("rewritten from"), "{prompt}");
    assert!(prompt.contains("What this changes"), "{prompt}");
}
