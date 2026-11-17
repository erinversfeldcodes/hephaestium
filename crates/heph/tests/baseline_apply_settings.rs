//! `heph baseline apply --settings` as a person or a workflow meets it.
//!
//! GitHub is stood in for by a `gh` that records every write. Nothing here is
//! run against a real repository.

mod common;

use std::process::Output;

use common::github::*;
use common::{Scratch, compliant, conforms, heph_with_env, stderr, stdout};
use serde_json::{Value, json};

/// A public repository on which every setting the baseline asks for is off and
/// no ruleset exists.
fn bare_github(name: &str) -> Gh {
    let gh = Gh::new(name);
    let mut repo = repo_json();
    repo["delete_branch_on_merge"] = json!(false);
    repo["security_and_analysis"]["secret_scanning"]["status"] = json!("disabled");
    repo["security_and_analysis"]["secret_scanning_push_protection"]["status"] = json!("disabled");
    gh.respond(&endpoint(""), &repo);
    gh.fail(
        &endpoint("/vulnerability-alerts"),
        "gh: Not Found (HTTP 404)",
    );
    gh.respond(
        &endpoint("/automated-security-fixes"),
        &json!({"enabled": false, "paused": false}),
    );
    gh.respond(
        &endpoint("/code-scanning/default-setup"),
        &json!({"state": "not-configured"}),
    );
    gh.respond(&endpoint("/code-scanning/analyses?per_page=1"), &json!([]));
    gh.respond(&endpoint("/rulesets"), &json!([]));
    gh
}

fn path_with(gh: &Gh) -> String {
    format!(
        "{}:{}",
        gh.bin().display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn apply_settings(scratch: &Scratch, gh: &Gh, extra: &[&str]) -> (Output, Value) {
    let mut args = vec!["baseline", "apply", "--settings", "--repo", REPO];
    args.extend_from_slice(extra);
    let output = heph_with_env(
        &scratch.0,
        &args,
        &[
            ("PATH", path_with(gh).as_str()),
            ("HEPH_TEST_GH_DIR", &gh.scratch.0.to_string_lossy()),
        ],
    );
    if matches!(output.status.code(), Some(2 | 3)) {
        return (output, Value::Null);
    }
    let text = stdout(&output);
    let result: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "stdout should be JSON ({error}): {text:?}\nstderr: {}",
            stderr(&output)
        )
    });
    conforms(&result);
    (output, result)
}

fn changes(result: &Value) -> Vec<Value> {
    result["report"]["changes"]
        .as_array()
        .unwrap_or_else(|| panic!("no changes in {result}"))
        .clone()
}

fn items_of(change: &Value) -> Vec<String> {
    change["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item.as_str().expect("an id").to_owned())
        .collect()
}

fn change_for<'a>(all: &'a [Value], item: &str) -> &'a Value {
    all.iter()
        .find(|change| items_of(change).iter().any(|id| id == item))
        .unwrap_or_else(|| panic!("no change for {item}: {all:?}"))
}

fn write_of(gh: &Gh, method: &str, endpoint_path: &str) -> Option<Value> {
    gh.writes()
        .into_iter()
        .find(|(m, e, _)| m == method && e == &endpoint(endpoint_path))
        .map(|(_, _, body)| {
            if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&body).expect("a JSON body")
            }
        })
}

#[test]
fn nothing_is_changed_without_yes_and_the_plan_shows_the_requests() {
    let scratch = repository("as-dry");
    let gh = bare_github("as-dry-gh");

    let (output, result) = apply_settings(&scratch, &gh, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(gh.writes(), vec![], "a write was made without --yes");
    assert_eq!(result["gate"], "baseline-apply-settings");
    assert_eq!(result["report"]["dry_run"], true);
    let all = changes(&result);
    assert!(!all.is_empty());
    for change in &all {
        assert_eq!(change["outcome"], "planned", "{change}");
        assert!(change["method"].is_string() && change["endpoint"].is_string());
    }
    let ruleset = change_for(&all, "branch-pull-request");
    assert_eq!(items_of(ruleset).len(), 7, "{ruleset}");
    assert_eq!(ruleset["body"]["name"], "default branch");
}

#[test]
fn with_yes_each_setting_is_one_request_with_the_right_method_endpoint_and_body() {
    let scratch = repository("as-yes");
    let gh = bare_github("as-yes-gh");

    let (output, result) = apply_settings(&scratch, &gh, &["--yes"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(result["report"]["dry_run"], false);
    for change in changes(&result) {
        assert_eq!(change["outcome"], "applied", "{change}");
    }
    let patches: Vec<Value> = gh
        .writes()
        .into_iter()
        .filter(|(m, e, _)| m == "PATCH" && e == &endpoint(""))
        .map(|(_, _, body)| serde_json::from_str(&body).expect("JSON"))
        .collect();
    for expected in [
        json!({"security_and_analysis": {"secret_scanning": {"status": "enabled"}}}),
        json!({"security_and_analysis": {"secret_scanning_push_protection": {"status": "enabled"}}}),
        json!({"delete_branch_on_merge": true}),
    ] {
        assert!(
            patches.contains(&expected),
            "missing {expected}: {patches:?}"
        );
    }
    assert_eq!(patches.len(), 3, "{patches:?}");
    assert_eq!(
        write_of(&gh, "PUT", "/vulnerability-alerts"),
        Some(Value::Null)
    );
    assert_eq!(
        write_of(&gh, "PUT", "/automated-security-fixes"),
        Some(Value::Null)
    );
    assert!(write_of(&gh, "POST", "/rulesets").is_some());
    assert_eq!(gh.writes().len(), 6, "{:?}", gh.writes());
}

#[test]
fn the_ruleset_it_creates_satisfies_every_ruleset_item() {
    let scratch = repository("as-roundtrip");
    let gh = bare_github("as-roundtrip-gh");
    apply_settings(&scratch, &gh, &["--yes"]);
    let mut created = write_of(&gh, "POST", "/rulesets").expect("a ruleset was created");
    created["id"] = json!(99);
    install_rulesets(&gh, &[created]);

    let output = heph_with_env(
        &scratch.0,
        &["baseline", "check", "--settings", "--repo", REPO],
        &[
            ("PATH", path_with(&gh).as_str()),
            ("HEPH_TEST_GH_DIR", &gh.scratch.0.to_string_lossy()),
        ],
    );

    let result: Value = serde_json::from_str(&stdout(&output)).expect("JSON");
    for item in [
        "branch-pull-request",
        "branch-approval",
        "branch-required-checks",
        "branch-linear-history",
        "branch-no-force-push",
        "branch-no-deletion",
        "branch-bypass",
    ] {
        assert_eq!(
            result["report"]["items"][item]["outcome"], "satisfied",
            "{item}: {result}"
        );
    }
}

#[test]
fn the_ruleset_carries_the_checks_and_the_bypass_the_answers_name() {
    let answers = ANSWERS
        .replace("[\"ci\"]", "[\"fmt + clippy\", \"test\"]")
        .replace("admin-pull-request", "none");
    let scratch = repository("as-answers");
    scratch.write(".athenaeum/baseline.toml", &answers);
    let gh = bare_github("as-answers-gh");

    apply_settings(&scratch, &gh, &["--yes"]);

    let body = write_of(&gh, "POST", "/rulesets").expect("a ruleset");
    assert_eq!(body["bypass_actors"], json!([]));
    let checks = body["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|rule| rule["type"] == "required_status_checks")
        .expect("a checks rule");
    assert_eq!(
        checks["parameters"]["required_status_checks"],
        json!([{"context": "fmt + clippy"}, {"context": "test"}])
    );

    let scratch = repository("as-answers-admin");
    let gh = bare_github("as-answers-admin-gh");
    apply_settings(&scratch, &gh, &["--yes"]);
    let body = write_of(&gh, "POST", "/rulesets").expect("a ruleset");
    assert_eq!(body["bypass_actors"], admin_bypass());
}

#[test]
fn satisfied_items_make_no_request() {
    let scratch = repository("as-satisfied");
    let gh = compliant_github("as-satisfied-gh");

    let (output, result) = apply_settings(&scratch, &gh, &["--yes"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(gh.writes(), vec![]);
    assert_eq!(changes(&result).len(), 0);
}

#[test]
fn an_excepted_item_makes_no_request() {
    let scratch = repository("as-excepted");
    scratch.write(
        ".athenaeum/baseline.toml",
        &format!(
            "{ANSWERS}\n[exceptions.security-dependabot-alerts]\nreason = \"Handled elsewhere.\"\n"
        ),
    );
    let gh = bare_github("as-excepted-gh");

    apply_settings(&scratch, &gh, &["--yes"]);

    assert_eq!(write_of(&gh, "PUT", "/vulnerability-alerts"), None);
    assert!(write_of(&gh, "PUT", "/automated-security-fixes").is_some());
}

#[test]
fn a_private_repository_is_not_asked_for_what_applies_to_public_ones() {
    let scratch = repository("as-private");
    let gh = bare_github("as-private-gh");
    let mut repo = repo_json();
    repo["visibility"] = json!("private");
    repo["private"] = json!(true);
    repo["delete_branch_on_merge"] = json!(true);
    repo["security_and_analysis"]["secret_scanning"]["status"] = json!("disabled");
    repo["security_and_analysis"]["secret_scanning_push_protection"]["status"] = json!("disabled");
    gh.respond(&endpoint(""), &repo);

    apply_settings(&scratch, &gh, &["--yes"]);

    assert!(
        gh.writes()
            .iter()
            .all(|(m, e, _)| !(m == "PATCH" && e == &endpoint(""))),
        "{:?}",
        gh.writes()
    );
}

#[test]
fn an_existing_ruleset_is_never_modified_and_the_gap_is_reported() {
    let scratch = repository("as-existing");
    let gh = bare_github("as-existing-gh");
    let mut existing = good_ruleset();
    let rules: Vec<Value> = existing["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .filter(|rule| rule["type"] != "required_linear_history")
        .cloned()
        .collect();
    existing["rules"] = json!(rules);
    install_rulesets(&gh, &[existing]);

    let (_, result) = apply_settings(&scratch, &gh, &["--yes"]);

    assert!(
        gh.writes().iter().all(|(_, e, _)| !e.contains("/rulesets")),
        "a ruleset was written: {:?}",
        gh.writes()
    );
    let skipped = result["report"]["skipped"].as_array().expect("skipped");
    let gap = skipped
        .iter()
        .find(|entry| entry["item"] == "branch-linear-history")
        .unwrap_or_else(|| panic!("the gap is not reported: {result}"));
    assert!(
        gap["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("existing ruleset")
    );
}

#[test]
fn code_scanning_and_the_description_are_not_applied_and_say_why() {
    let scratch = repository("as-choices");
    let gh = bare_github("as-choices-gh");
    let mut repo = repo_json();
    repo["description"] = Value::Null;
    gh.respond(&endpoint(""), &repo);

    let (_, result) = apply_settings(&scratch, &gh, &["--yes"]);

    let skipped = result["report"]["skipped"].as_array().expect("skipped");
    for item in ["security-code-scanning", "repository-description"] {
        let entry = skipped
            .iter()
            .find(|entry| entry["item"] == item)
            .unwrap_or_else(|| panic!("{item} is not reported: {result}"));
        assert!(!entry["reason"].as_str().unwrap_or_default().is_empty());
    }
    assert!(write_of(&gh, "PATCH", "/code-scanning/default-setup").is_none());
}

#[test]
fn a_request_that_fails_is_reported_and_the_others_still_run() {
    let scratch = repository("as-failure");
    let gh = bare_github("as-failure-gh");
    gh.fail_write(
        "PUT",
        &endpoint("/vulnerability-alerts"),
        "gh: Resource not accessible by integration (HTTP 403)",
    );

    let (output, result) = apply_settings(&scratch, &gh, &["--yes"]);

    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
    assert_eq!(result["outcome"], "failed");
    let all = changes(&result);
    let failed = change_for(&all, "security-dependabot-alerts");
    assert_eq!(failed["outcome"], "failed");
    assert!(
        failed["message"]
            .as_str()
            .unwrap_or_default()
            .contains("403")
    );
    assert_eq!(
        change_for(&all, "security-dependabot-updates")["outcome"],
        "applied"
    );
    assert!(
        write_of(&gh, "POST", "/rulesets").is_some(),
        "the rest still ran"
    );
}

#[test]
fn it_could_not_be_applied_without_answers_gh_or_a_readable_repository_and_changes_nothing() {
    // No answers file.
    let scratch = compliant("as-no-answers");
    let gh = bare_github("as-no-answers-gh");
    let (output, _) = apply_settings(&scratch, &gh, &["--yes"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(stderr(&output).contains("answers"));
    assert_eq!(gh.writes(), vec![]);

    // No gh.
    let scratch = repository("as-no-gh");
    let empty = Scratch::new("as-no-gh-path");
    let output = heph_with_env(
        &scratch.0,
        &["baseline", "apply", "--settings", "--repo", REPO, "--yes"],
        &[("PATH", &empty.0.to_string_lossy())],
    );
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");

    // The repository cannot be read.
    let gh = bare_github("as-unreadable-gh");
    gh.fail(&endpoint(""), "gh: Bad credentials (HTTP 401)");
    let (output, _) = apply_settings(&scratch, &gh, &["--yes"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert_eq!(gh.writes(), vec![]);
}

#[test]
fn every_request_goes_to_the_named_repository_and_the_working_tree_is_untouched() {
    let scratch = repository("as-scope");
    let gh = bare_github("as-scope-gh");
    let before = std::fs::read_dir(&scratch.0).expect("a directory").count();

    apply_settings(&scratch, &gh, &["--yes"]);

    assert!(!gh.writes().is_empty());
    for (_, endpoint_path, _) in gh.writes() {
        assert!(
            endpoint_path.starts_with("repos/owner/name"),
            "{endpoint_path}"
        );
    }
    assert_eq!(
        std::fs::read_dir(&scratch.0).expect("a directory").count(),
        before
    );
    assert!(!scratch.exists("README.md") || scratch.read("README.md") == "x\n");
}

#[test]
fn yes_without_settings_is_a_usage_error() {
    let scratch = repository("as-usage");

    let output = common::heph_in(&scratch.0, &["baseline", "apply", "--repo", REPO, "--yes"]);

    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}
