//! A completeness evaluation reads runs, caches, pull requests and issues, which are on
//! GitHub, so it is given a read-only token of its own, as `GH_TOKEN`. It is a different
//! credential from the one that pushes. Only this kind of workload is given it, and the
//! token that writes never reaches it.

mod common;

use std::os::unix::fs::PermissionsExt as _;

use common::{Scratch, manifest, run_with_env, source, workload};
use contracts::v1alpha1::event::Kind;
use serde_json::{Value, json};

/// What the agent sees: `GH_TOKEN`, then the write token, then the read token by its own
/// name, each or `none`.
const SEES: &str = r#"printf '{"agentMessage":{"text":"%s|%s|%s"}}\n' "${GH_TOKEN:-none}" "${HEPH_GITHUB_TOKEN:-none}" "${HEPH_GITHUB_READ_TOKEN:-none}""#;

fn arguments() -> Vec<String> {
    vec![
        "--agent-command".to_owned(),
        json!(["sh", "-c", SEES]).to_string(),
    ]
}

fn as_args(arguments: &[String]) -> Vec<&str> {
    arguments.iter().map(String::as_str).collect()
}

/// What the agent said it saw, for a workload that is an evaluation or is not.
fn seen(name: &str, evaluation: bool, environment: &[(&str, &str)]) -> String {
    let scratch = Scratch::new(name);
    let (_, commit) = source(&scratch, &manifest(&[]));
    let mut value: Value = workload(&commit);
    if evaluation {
        value["completeness"] = json!({ "skillDocument": "the skill" });
    }
    let arguments = arguments();

    let ran = run_with_env(scratch, &value, None, &as_args(&arguments), environment);

    let events = ran.events();
    let message = events
        .iter()
        .find_map(|event| match event.kind.as_ref() {
            Some(Kind::AgentMessage(message)) => Some(message.text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the agent said nothing: {}", ran.stderr()));
    message
}

#[test]
fn an_evaluation_is_given_the_read_token_as_gh_token_and_nothing_else_by_name() {
    let said = seen(
        "token-evaluation",
        true,
        &[("HEPH_GITHUB_READ_TOKEN", "read-only-token")],
    );

    assert_eq!(said, "read-only-token|none|none");
}

#[test]
fn the_token_that_writes_never_reaches_an_evaluation() {
    let said = seen(
        "token-never-the-write-token",
        true,
        &[
            ("HEPH_GITHUB_READ_TOKEN", "read-only-token"),
            ("HEPH_GITHUB_TOKEN", "write-token"),
        ],
    );

    assert_eq!(said, "read-only-token|none|none");
    assert!(!said.contains("write-token"));
}

#[test]
fn a_workload_that_is_not_an_evaluation_is_never_given_the_read_token() {
    let said = seen(
        "token-ordinary",
        false,
        &[("HEPH_GITHUB_READ_TOKEN", "read-only-token")],
    );

    assert_eq!(said, "none|none|none");
}

#[test]
fn an_evaluation_with_no_read_token_has_none() {
    let said = seen("token-absent", true, &[]);

    assert_eq!(said, "none|none|none");
}

#[test]
fn the_read_token_can_be_kept_in_a_file_and_is_still_only_the_evaluations() {
    let scratch = Scratch::new("token-file");
    let file = scratch.write("read-token", "token-from-a-file\n");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("to chmod");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let mut value = workload(&commit);
    value["completeness"] = json!({ "skillDocument": "the skill" });
    let arguments = arguments();
    let path = file.display().to_string();

    let ran = run_with_env(
        scratch,
        &value,
        None,
        &as_args(&arguments),
        &[("HEPH_GITHUB_READ_TOKEN_FILE", path.as_str())],
    );

    let said = ran
        .events()
        .iter()
        .find_map(|event| match event.kind.as_ref() {
            Some(Kind::AgentMessage(message)) => Some(message.text.clone()),
            _ => None,
        })
        .expect("the agent said something");
    assert_eq!(said, "token-from-a-file|none|none");
}
