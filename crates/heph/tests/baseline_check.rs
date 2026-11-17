//! `heph baseline check` as a person or a workflow meets it.
//!
//! Each test builds a small repository in a scratch directory, runs the real
//! binary against it, and reads the result. Every result is also held to the
//! pinned Athenaeum gate result schema, whatever else a test checks.

mod common;

use std::process::Output;

use common::{CLEAN_WORKFLOW, Scratch, compliant, conforms, heph_in, stderr, stdout};
use serde_json::Value;

fn check(scratch: &Scratch, extra: &[&str]) -> (Output, Value) {
    let mut args = vec!["baseline", "check"];
    args.extend_from_slice(extra);
    let output = heph_in(&scratch.0, &args);
    let text = stdout(&output);
    if output.status.code() == Some(3) {
        return (output, Value::Null);
    }
    assert_eq!(
        text.lines().count(),
        1,
        "stdout should be one line of JSON, but is: {text:?}\nstderr: {}",
        stderr(&output)
    );
    let result: Value = serde_json::from_str(&text).expect("stdout to be JSON");
    conforms(&result);
    (output, result)
}

fn outcome<'a>(result: &'a Value, item: &str) -> &'a str {
    result["report"]["items"][item]["outcome"]
        .as_str()
        .unwrap_or_else(|| panic!("the report has no outcome for {item}: {result}"))
}

fn detail(result: &Value, item: &str) -> String {
    result["report"]["items"][item]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn workflow(scratch: &Scratch, name: &str, text: &str) {
    scratch.write(&format!(".github/workflows/{name}.yml"), text);
}

#[test]
fn a_repository_with_every_file_and_clean_workflows_passes() {
    let scratch = compliant("bl-clean");

    let (output, result) = check(&scratch, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(result["gate"], "baseline");
    assert_eq!(result["outcome"], "passed");
    for item in [
        "community-readme",
        "community-licence",
        "community-security-policy",
        "community-contributing",
        "community-code-of-conduct",
        "community-issue-templates",
        "community-pull-request-template",
        "community-code-owners",
        "dependencies-dependabot-config",
        "ci-actions-pinned",
        "ci-permissions-declared",
        "ci-job-timeouts",
    ] {
        assert_eq!(outcome(&result, item), "satisfied", "{item}: {result}");
    }
}

#[test]
fn a_missing_required_file_fails_and_is_named() {
    let scratch = compliant("bl-missing-required");
    std::fs::remove_file(scratch.0.join("README.md")).expect("to remove the README");

    let (output, result) = check(&scratch, &[]);

    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
    assert_eq!(result["outcome"], "failed");
    assert_eq!(outcome(&result, "community-readme"), "unsatisfied");
    assert!(
        stderr(&output).contains("community-readme"),
        "stderr should name the item: {}",
        stderr(&output)
    );
}

#[test]
fn a_missing_recommended_file_is_reported_and_does_not_fail() {
    let scratch = compliant("bl-missing-recommended");
    std::fs::remove_file(scratch.0.join("CODE_OF_CONDUCT.md")).expect("to remove the file");

    let (output, result) = check(&scratch, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(result["outcome"], "passed");
    assert_eq!(outcome(&result, "community-code-of-conduct"), "unsatisfied");
}

#[test]
fn a_glob_is_satisfied_by_a_file_inside_the_directory() {
    let scratch = compliant("bl-glob");
    std::fs::remove_dir_all(scratch.0.join(".github/ISSUE_TEMPLATE")).expect("to remove");
    let (_, without) = check(&scratch, &[]);
    assert_eq!(
        outcome(&without, "community-issue-templates"),
        "unsatisfied"
    );

    scratch.write(".github/ISSUE_TEMPLATE/feature.md", "x\n");
    let (_, with) = check(&scratch, &[]);

    assert_eq!(outcome(&with, "community-issue-templates"), "satisfied");
}

#[test]
fn an_exception_turns_an_unsatisfied_item_into_excepted_with_its_reason() {
    let scratch = compliant("bl-excepted");
    std::fs::remove_file(scratch.0.join("SECURITY.md")).expect("to remove the file");
    scratch.write(
        ".athenaeum/baseline.toml",
        "schema_version = 0\nlicence = \"mit\"\ncontact = \"security@example.org\"\n\
         required_checks = [\"ci\"]\ndefault_branch_bypass = \"none\"\n\n\
         [exceptions.community-security-policy]\nreason = \"Reported through the issue tracker.\"\n",
    );

    let (output, result) = check(&scratch, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(outcome(&result, "community-security-policy"), "excepted");
    assert_eq!(
        result["report"]["items"]["community-security-policy"]["reason"],
        "Reported through the issue tracker."
    );
}

#[test]
fn an_exception_for_an_item_that_does_not_exist_is_an_error() {
    let scratch = compliant("bl-unknown-exception");
    scratch.write(
        ".athenaeum/baseline.toml",
        "schema_version = 0\nlicence = \"mit\"\ncontact = \"security@example.org\"\n\
         required_checks = [\"ci\"]\ndefault_branch_bypass = \"none\"\n\n\
         [exceptions.community-readmee]\nreason = \"A typo.\"\n",
    );

    let output = heph_in(&scratch.0, &["baseline", "check"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("community-readmee"),
        "stderr should name the unknown id: {}",
        stderr(&output)
    );
}

#[test]
fn answers_that_break_their_schema_are_an_error() {
    let scratch = compliant("bl-bad-answers");
    scratch.write(
        ".athenaeum/baseline.toml",
        "schema_version = 0\nlicence = \"mit\"\n",
    );

    let output = heph_in(&scratch.0, &["baseline", "check"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "");
}

#[test]
fn without_an_answers_file_there_are_no_exceptions_and_the_report_says_so() {
    let scratch = compliant("bl-no-answers");

    let (_, result) = check(&scratch, &[]);

    assert_eq!(result["report"]["answers"], "absent");
}

#[test]
fn an_action_is_pinned_when_it_names_a_version_a_commit_a_local_path_or_a_digest() {
    let scratch = compliant("bl-pinned");
    workflow(
        &scratch,
        "ci",
        "name: ci
on: [push]
permissions: {}
jobs:
  test:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@v4
      - uses: owner/tool@1.99.1
      - uses: owner/other@v2.3
      - uses: owner/pinned@0123456789abcdef0123456789abcdef01234567
      - uses: ./.github/actions/local
      - uses: docker://alpine@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
",
    );

    let (_, result) = check(&scratch, &[]);

    assert_eq!(
        outcome(&result, "ci-actions-pinned"),
        "satisfied",
        "{result}"
    );
}

#[test]
fn an_action_at_a_branch_is_unsatisfied_and_the_file_and_action_are_named() {
    for reference in ["main", "master", "stable", "release-1"] {
        let scratch = compliant("bl-unpinned");
        workflow(
            &scratch,
            "deploy",
            &format!(
                "name: deploy
on: [push]
permissions: {{}}
jobs:
  deploy:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - uses: owner/tool@{reference}
"
            ),
        );

        let (output, result) = check(&scratch, &[]);

        assert_eq!(output.status.code(), Some(1), "@{reference}");
        assert_eq!(
            outcome(&result, "ci-actions-pinned"),
            "unsatisfied",
            "@{reference}"
        );
        let message = detail(&result, "ci-actions-pinned");
        assert!(
            message.contains("deploy.yml")
                && message.contains("owner/tool@")
                && message.contains(reference),
            "the detail should name the file and the action: {message}"
        );
    }
}

#[test]
fn a_docker_image_without_a_digest_and_an_action_with_no_ref_are_unsatisfied() {
    for step in ["docker://alpine:3.20", "owner/tool"] {
        let scratch = compliant("bl-unpinned-forms");
        workflow(
            &scratch,
            "deploy",
            &format!(
                "name: deploy
on: [push]
permissions: {{}}
jobs:
  deploy:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - uses: {step}
"
            ),
        );

        let (_, result) = check(&scratch, &[]);

        assert_eq!(
            outcome(&result, "ci-actions-pinned"),
            "unsatisfied",
            "{step}"
        );
    }
}

#[test]
fn permissions_are_satisfied_at_the_workflow_or_on_every_job_and_not_otherwise() {
    let jobs = |permissions: &str, second: &str| {
        format!(
            "name: p
on: [push]
{permissions}jobs:
  a:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    permissions:
      contents: read
    steps:
      - run: echo a
  b:
    runs-on: ubuntu-latest
    timeout-minutes: 5
{second}    steps:
      - run: echo b
"
        )
    };

    for (label, text, expected) in [
        (
            "workflow level",
            jobs("permissions:\n  contents: read\n", ""),
            "satisfied",
        ),
        (
            "every job",
            jobs("", "    permissions:\n      contents: read\n"),
            "satisfied",
        ),
        ("one job without", jobs("", ""), "unsatisfied"),
    ] {
        let scratch = compliant("bl-permissions");
        workflow(&scratch, "ci", &text);

        let (_, result) = check(&scratch, &[]);

        assert_eq!(
            outcome(&result, "ci-permissions-declared"),
            expected,
            "{label}"
        );
    }
}

#[test]
fn a_job_without_a_timeout_is_reported_but_a_reusable_workflow_call_is_exempt() {
    let scratch = compliant("bl-timeouts");
    workflow(
        &scratch,
        "ci",
        "name: t
on: [push]
permissions: {}
jobs:
  a:
    runs-on: ubuntu-latest
    steps:
      - run: echo a
",
    );
    let (output, result) = check(&scratch, &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "recommended, so it does not fail"
    );
    assert_eq!(outcome(&result, "ci-job-timeouts"), "unsatisfied");

    workflow(
        &scratch,
        "ci",
        "name: t
on: [push]
permissions: {}
jobs:
  call:
    uses: owner/repo/.github/workflows/shared.yml@v1
",
    );
    let (_, exempt) = check(&scratch, &[]);
    assert_eq!(outcome(&exempt, "ci-job-timeouts"), "satisfied", "{exempt}");
}

#[test]
fn with_no_workflows_the_workflow_rules_do_not_apply() {
    let scratch = compliant("bl-no-workflows");
    std::fs::remove_file(scratch.0.join(".github/workflows/ci.yml")).expect("to remove");

    let (output, result) = check(&scratch, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    for item in [
        "ci-actions-pinned",
        "ci-permissions-declared",
        "ci-job-timeouts",
    ] {
        assert_eq!(outcome(&result, item), "not_applicable", "{item}");
    }
}

#[test]
fn settings_and_ruleset_items_are_not_checked_and_never_fail() {
    let scratch = compliant("bl-not-checked");

    let (output, result) = check(&scratch, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    for item in [
        "security-dependabot-alerts",
        "security-code-scanning",
        "branch-pull-request",
        "branch-bypass",
    ] {
        assert_eq!(outcome(&result, item), "not_checked", "{item}");
    }
}

#[test]
fn a_workflow_that_is_not_yaml_is_an_error_and_names_the_file() {
    let scratch = compliant("bl-bad-yaml");
    workflow(&scratch, "broken", "jobs: [unclosed\n  - : :\n");

    let output = heph_in(&scratch.0, &["baseline", "check"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("broken.yml"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn root_chooses_the_repository_to_read() {
    let scratch = compliant("bl-root");
    let elsewhere = Scratch::new("bl-root-cwd");

    let output = heph_in(
        &elsewhere.0,
        &["baseline", "check", "--root", &scratch.0.to_string_lossy()],
    );

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    let scratch = compliant("bl-usage");

    let output = heph_in(&scratch.0, &["baseline", "check", "--nope"]);

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
}

#[test]
fn a_directory_that_holds_what_a_build_left_is_not_walked() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = compliant("bl-skipped-directories");
    // A directory that cannot be read is an error if the walk enters it, so a
    // check that passes shows it did not.
    let locked = scratch.0.join("node_modules/pkg");
    std::fs::create_dir_all(&locked).expect("to create the directory");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
        .expect("to lock the directory");

    let output = heph_in(&scratch.0, &["baseline", "check"]);

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
        .expect("to unlock the directory");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
}
