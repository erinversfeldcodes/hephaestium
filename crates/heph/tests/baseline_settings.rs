//! `heph baseline check --settings` as a person or a workflow meets it.
//!
//! GitHub is stood in for by a `gh` program first on the path, which answers
//! from recorded responses shaped like GitHub's real ones and logs what it was
//! asked. That proves the logic against those shapes. It does not prove that
//! GitHub answers that way: the branch is also run against real repositories.

mod common;

use std::path::Path;
use std::process::Output;

use common::github::*;
use common::{Scratch, compliant, conforms, heph_with_env, stderr, stdout};
use serde_json::{Value, json};

fn run(scratch: &Scratch, gh: &Gh, extra: &[&str]) -> (Output, Value) {
    let path = format!(
        "{}:{}",
        gh.bin().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut args = vec!["baseline", "check", "--settings", "--repo", REPO];
    args.extend_from_slice(extra);
    let output = heph_with_env(
        &scratch.0,
        &args,
        &[
            ("PATH", path.as_str()),
            ("HEPH_TEST_GH_DIR", &gh.scratch.0.to_string_lossy()),
        ],
    );
    if output.status.code() == Some(3) {
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

fn outcome<'a>(result: &'a Value, item: &str) -> &'a str {
    result["report"]["items"][item]["outcome"]
        .as_str()
        .unwrap_or_else(|| panic!("no outcome for {item}: {result}"))
}

fn detail(result: &Value, item: &str) -> String {
    result["report"]["items"][item]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

const SETTINGS: [&str; 7] = [
    "security-secret-scanning",
    "security-push-protection",
    "security-dependabot-alerts",
    "security-dependabot-updates",
    "security-code-scanning",
    "repository-delete-branch-on-merge",
    "repository-description",
];

const RULESET_ITEMS: [&str; 7] = [
    "branch-pull-request",
    "branch-approval",
    "branch-required-checks",
    "branch-linear-history",
    "branch-no-force-push",
    "branch-no-deletion",
    "branch-bypass",
];

fn only_unsatisfied(result: &Value) -> Vec<String> {
    let mut found: Vec<String> = result["report"]["items"]
        .as_object()
        .expect("items")
        .iter()
        .filter(|(_, item)| item["outcome"] == "unsatisfied")
        .map(|(id, _)| id.clone())
        .collect();
    found.sort();
    found
}

#[test]
fn a_repository_that_satisfies_every_setting_and_ruleset_item_passes_with_nothing_unchecked() {
    let scratch = repository("bs-all");
    let gh = compliant_github("bs-all-gh");

    let (output, result) = run(&scratch, &gh, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    for item in SETTINGS.iter().chain(RULESET_ITEMS.iter()) {
        assert_eq!(outcome(&result, item), "satisfied", "{item}: {result}");
    }
    assert_eq!(result["report"]["summary"]["not_checked"], 0);
}

#[test]
fn each_setting_that_is_off_is_unsatisfied_and_only_that_one() {
    type Break = fn(&Gh);
    let cases: [(&str, Break); 7] = [
        ("security-secret-scanning", |gh| {
            let mut repo = repo_json();
            repo["security_and_analysis"]["secret_scanning"]["status"] = json!("disabled");
            gh.respond(&endpoint(""), &repo);
        }),
        ("security-push-protection", |gh| {
            let mut repo = repo_json();
            repo["security_and_analysis"]["secret_scanning_push_protection"]["status"] =
                json!("disabled");
            gh.respond(&endpoint(""), &repo);
        }),
        ("security-dependabot-alerts", |gh| {
            gh.fail(
                &endpoint("/vulnerability-alerts"),
                "gh: Not Found (HTTP 404)",
            );
        }),
        ("security-dependabot-updates", |gh| {
            gh.respond(
                &endpoint("/automated-security-fixes"),
                &json!({"enabled": false, "paused": false}),
            );
        }),
        ("security-code-scanning", |gh| {
            gh.respond(
                &endpoint("/code-scanning/default-setup"),
                &json!({"state": "not-configured"}),
            );
        }),
        ("repository-delete-branch-on-merge", |gh| {
            let mut repo = repo_json();
            repo["delete_branch_on_merge"] = json!(false);
            gh.respond(&endpoint(""), &repo);
        }),
        ("repository-description", |gh| {
            let mut repo = repo_json();
            repo["description"] = Value::Null;
            gh.respond(&endpoint(""), &repo);
        }),
    ];
    for (item, break_it) in cases {
        let scratch = repository("bs-off");
        let gh = compliant_github("bs-off-gh");
        break_it(&gh);

        let (_, result) = run(&scratch, &gh, &[]);

        assert_eq!(
            only_unsatisfied(&result),
            vec![item.to_owned()],
            "{item}: {result}"
        );
    }
}

#[test]
fn a_required_setting_that_is_off_fails_the_check_and_a_recommended_one_does_not() {
    let scratch = repository("bs-severity");
    let gh = compliant_github("bs-severity-gh");
    gh.fail(
        &endpoint("/vulnerability-alerts"),
        "gh: Not Found (HTTP 404)",
    );
    let (output, _) = run(&scratch, &gh, &[]);
    assert_eq!(output.status.code(), Some(1));

    let gh = compliant_github("bs-severity-gh2");
    gh.respond(
        &endpoint("/code-scanning/default-setup"),
        &json!({"state": "not-configured"}),
    );
    let (output, _) = run(&scratch, &gh, &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "recommended, so it does not fail"
    );
}

#[test]
fn code_scanning_is_satisfied_by_an_analysis_when_default_setup_is_not_used() {
    let scratch = repository("bs-analyses");
    let gh = compliant_github("bs-analyses-gh");
    gh.respond(
        &endpoint("/code-scanning/default-setup"),
        &json!({"state": "not-configured"}),
    );
    gh.respond(
        &endpoint("/code-scanning/analyses?per_page=1"),
        &json!([{"id": 1}]),
    );

    let (_, result) = run(&scratch, &gh, &[]);

    assert_eq!(
        outcome(&result, "security-code-scanning"),
        "satisfied",
        "{result}"
    );
}

#[test]
fn a_private_repository_is_not_asked_for_what_applies_to_public_ones() {
    let scratch = repository("bs-private");
    let gh = compliant_github("bs-private-gh");
    let mut repo = repo_json();
    repo["visibility"] = json!("private");
    repo["private"] = json!(true);
    repo["security_and_analysis"]["secret_scanning"]["status"] = json!("disabled");
    gh.respond(&endpoint(""), &repo);

    let (_, result) = run(&scratch, &gh, &[]);

    assert_eq!(
        outcome(&result, "security-secret-scanning"),
        "not_applicable"
    );
    assert_eq!(
        outcome(&result, "security-push-protection"),
        "not_applicable"
    );
}

#[test]
fn a_field_github_does_not_report_is_not_checked_and_never_unsatisfied() {
    let scratch = repository("bs-unreported");
    let gh = compliant_github("bs-unreported-gh");
    let mut repo = repo_json();
    repo.as_object_mut()
        .expect("object")
        .remove("security_and_analysis");
    gh.respond(&endpoint(""), &repo);
    gh.fail(
        &endpoint("/vulnerability-alerts"),
        "gh: Resource not accessible (HTTP 403)",
    );
    gh.fail(&endpoint("/rulesets"), "gh: Forbidden (HTTP 403)");

    let (output, result) = run(&scratch, &gh, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    for item in [
        "security-secret-scanning",
        "security-push-protection",
        "security-dependabot-alerts",
        "branch-pull-request",
        "branch-bypass",
    ] {
        assert_eq!(outcome(&result, item), "not_checked", "{item}");
        assert!(!detail(&result, item).is_empty(), "{item} should say why");
    }
}

#[test]
fn with_no_rulesets_every_ruleset_item_is_unsatisfied() {
    let scratch = repository("bs-no-rulesets");
    let gh = compliant_github("bs-no-rulesets-gh");
    gh.respond(&endpoint("/rulesets"), &json!([]));

    let (output, result) = run(&scratch, &gh, &[]);

    assert_eq!(output.status.code(), Some(1));
    let mut expected: Vec<String> = RULESET_ITEMS.iter().map(|s| (*s).to_owned()).collect();
    expected.sort();
    assert_eq!(only_unsatisfied(&result), expected);
}

#[test]
fn each_ruleset_rule_that_is_missing_is_unsatisfied_and_only_that_one() {
    let without = |kind: &str| -> Value {
        let mut base = good_ruleset();
        let rules: Vec<Value> = base["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .filter(|r| r["type"] != kind)
            .cloned()
            .collect();
        base["rules"] = json!(rules);
        base
    };
    for (kind, item) in [
        (
            "pull_request",
            vec!["branch-approval", "branch-pull-request"],
        ),
        ("required_linear_history", vec!["branch-linear-history"]),
        ("non_fast_forward", vec!["branch-no-force-push"]),
        ("deletion", vec!["branch-no-deletion"]),
        ("required_status_checks", vec!["branch-required-checks"]),
    ] {
        let scratch = repository("bs-missing-rule");
        let gh = compliant_github("bs-missing-rule-gh");
        install_rulesets(&gh, &[without(kind)]);

        let (_, result) = run(&scratch, &gh, &[]);

        let mut expected: Vec<String> = item.iter().map(|s| (*s).to_owned()).collect();
        expected.sort();
        assert_eq!(only_unsatisfied(&result), expected, "without {kind}");
    }
}

#[test]
fn a_pull_request_rule_that_asks_for_no_approval_does_not_satisfy_the_approval_item() {
    let scratch = repository("bs-zero-approvals");
    let gh = compliant_github("bs-zero-approvals-gh");
    let mut rs = good_ruleset();
    rs["rules"] = json!([
        rule("deletion"),
        rule("non_fast_forward"),
        rule("required_linear_history"),
        pull_request_rule(0),
        checks_rule(&["ci"])
    ]);
    install_rulesets(&gh, &[rs]);

    let (_, result) = run(&scratch, &gh, &[]);

    assert_eq!(
        only_unsatisfied(&result),
        vec!["branch-approval".to_owned()]
    );
    assert_eq!(outcome(&result, "branch-pull-request"), "satisfied");
}

#[test]
fn required_checks_must_include_every_check_the_answers_name() {
    let scratch = repository("bs-checks");
    scratch.write(
        ".athenaeum/baseline.toml",
        &ANSWERS.replace("[\"ci\"]", "[\"ci\", \"test\"]"),
    );
    let gh = compliant_github("bs-checks-gh");

    let (_, result) = run(&scratch, &gh, &[]);

    assert_eq!(outcome(&result, "branch-required-checks"), "unsatisfied");
    assert!(
        detail(&result, "branch-required-checks").contains("test"),
        "{result}"
    );

    let mut more = good_ruleset();
    more["rules"] = json!([checks_rule(&["ci", "test", "extra"])]);
    install_rulesets(&gh, &[more]);
    let (_, result) = run(&scratch, &gh, &[]);
    assert_eq!(
        outcome(&result, "branch-required-checks"),
        "satisfied",
        "a superset is enough"
    );
}

#[test]
fn the_bypass_must_be_exactly_what_the_answers_say() {
    let with_bypass = |bypass: Value| {
        let mut rs = good_ruleset();
        rs["bypass_actors"] = bypass;
        rs
    };
    let admin_always =
        json!([{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always"}]);
    let other_role =
        json!([{"actor_id": 2, "actor_type": "RepositoryRole", "bypass_mode": "pull_request"}]);
    let admin_and_app = json!([
        {"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "pull_request"},
        {"actor_id": 99, "actor_type": "Integration", "bypass_mode": "pull_request"}
    ]);
    for (label, answers_bypass, bypass, expected) in [
        (
            "admin, as asked",
            "admin-pull-request",
            admin_bypass(),
            "satisfied",
        ),
        ("none, as asked", "none", json!([]), "satisfied"),
        (
            "nobody, though admin is asked",
            "admin-pull-request",
            json!([]),
            "unsatisfied",
        ),
        (
            "admin, though none is asked",
            "none",
            admin_bypass(),
            "unsatisfied",
        ),
        (
            "admin by always",
            "admin-pull-request",
            admin_always,
            "unsatisfied",
        ),
        (
            "another role",
            "admin-pull-request",
            other_role,
            "unsatisfied",
        ),
        (
            "admin and an app",
            "admin-pull-request",
            admin_and_app,
            "unsatisfied",
        ),
    ] {
        let scratch = repository("bs-bypass");
        scratch.write(
            ".athenaeum/baseline.toml",
            &ANSWERS.replace("admin-pull-request", answers_bypass),
        );
        let gh = compliant_github("bs-bypass-gh");
        install_rulesets(&gh, &[with_bypass(bypass)]);

        let (_, result) = run(&scratch, &gh, &[]);

        assert_eq!(
            outcome(&result, "branch-bypass"),
            expected,
            "{label}: {result}"
        );
    }
}

#[test]
fn a_ruleset_that_is_disabled_for_tags_or_for_another_branch_does_not_count() {
    for (label, enforcement, target, include) in [
        ("disabled", "disabled", "branch", "~DEFAULT_BRANCH"),
        ("evaluate only", "evaluate", "branch", "~DEFAULT_BRANCH"),
        ("tags", "active", "tag", "~DEFAULT_BRANCH"),
        ("another branch", "active", "branch", "refs/heads/release"),
    ] {
        let scratch = repository("bs-not-counted");
        let gh = compliant_github("bs-not-counted-gh");
        let mut rs = good_ruleset();
        rs["enforcement"] = json!(enforcement);
        rs["target"] = json!(target);
        rs["conditions"]["ref_name"]["include"] = json!([include]);
        install_rulesets(&gh, &[rs]);

        let (output, _) = run(&scratch, &gh, &[]);

        assert_eq!(
            output.status.code(),
            Some(1),
            "{label} should not satisfy the items"
        );
    }
}

#[test]
fn a_ruleset_that_names_the_default_branch_counts_and_one_that_excludes_it_does_not() {
    let scratch = repository("bs-named");
    let gh = compliant_github("bs-named-gh");
    let mut rs = good_ruleset();
    rs["conditions"]["ref_name"]["include"] = json!(["refs/heads/main"]);
    install_rulesets(&gh, &[rs.clone()]);
    let (output, _) = run(&scratch, &gh, &[]);
    assert_eq!(output.status.code(), Some(0), "named by its full ref");

    rs["conditions"]["ref_name"]["include"] = json!(["~ALL"]);
    rs["conditions"]["ref_name"]["exclude"] = json!(["refs/heads/main"]);
    install_rulesets(&gh, &[rs]);
    let (output, _) = run(&scratch, &gh, &[]);
    assert_eq!(output.status.code(), Some(1), "excluded");
}

#[test]
fn rules_from_several_rulesets_add_up() {
    let scratch = repository("bs-several");
    let gh = compliant_github("bs-several-gh");
    let first = ruleset(
        1,
        "active",
        "branch",
        &["~DEFAULT_BRANCH"],
        &admin_bypass(),
        &[
            rule("deletion"),
            rule("non_fast_forward"),
            rule("required_linear_history"),
        ],
    );
    let second = ruleset(
        2,
        "active",
        "branch",
        &["~DEFAULT_BRANCH"],
        &admin_bypass(),
        &[pull_request_rule(1), checks_rule(&["ci"])],
    );
    install_rulesets(&gh, &[first, second]);

    let (output, result) = run(&scratch, &gh, &[]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {} {result}",
        stderr(&output)
    );
}

#[test]
fn without_an_answers_file_the_two_items_that_need_it_are_not_checked() {
    let scratch = compliant("bs-no-answers");
    let gh = compliant_github("bs-no-answers-gh");

    let (_, result) = run(&scratch, &gh, &[]);

    assert_eq!(outcome(&result, "branch-required-checks"), "not_checked");
    assert_eq!(outcome(&result, "branch-bypass"), "not_checked");
    assert!(
        detail(&result, "branch-bypass").contains("answers"),
        "{result}"
    );
    assert_eq!(outcome(&result, "branch-linear-history"), "satisfied");
}

#[test]
fn an_exception_applies_to_a_settings_item() {
    let scratch = repository("bs-exception");
    scratch.write(
        ".athenaeum/baseline.toml",
        &format!("{ANSWERS}\n[exceptions.security-code-scanning]\nreason = \"Not yet.\"\n"),
    );
    let gh = compliant_github("bs-exception-gh");
    gh.respond(
        &endpoint("/code-scanning/default-setup"),
        &json!({"state": "not-configured"}),
    );

    let (_, result) = run(&scratch, &gh, &[]);

    assert_eq!(outcome(&result, "security-code-scanning"), "excepted");
}

#[test]
fn classic_branch_protection_does_not_satisfy_a_ruleset_item() {
    let scratch = repository("bs-classic");
    let gh = compliant_github("bs-classic-gh");
    gh.respond(&endpoint("/rulesets"), &json!([]));
    gh.respond(
        &endpoint("/branches/main/protection"),
        &json!({"required_pull_request_reviews": {"required_approving_review_count": 1}}),
    );

    let (output, result) = run(&scratch, &gh, &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(outcome(&result, "branch-pull-request"), "unsatisfied");
}

#[test]
fn without_gh_the_check_could_not_be_made() {
    let scratch = repository("bs-no-gh");
    let empty = Scratch::new("bs-no-gh-path");

    let output = heph_with_env(
        &scratch.0,
        &["baseline", "check", "--settings", "--repo", REPO],
        &[("PATH", &empty.0.to_string_lossy())],
    );

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("gh"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn an_error_reading_the_repository_itself_could_not_be_made() {
    let scratch = repository("bs-repo-error");
    let gh = compliant_github("bs-repo-error-gh");
    gh.fail(&endpoint(""), "gh: Bad credentials (HTTP 401)");

    let (output, _) = run(&scratch, &gh, &[]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("401"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn the_repository_named_by_repo_is_the_one_asked_about() {
    let scratch = repository("bs-repo-flag");
    let gh = compliant_github("bs-repo-flag-gh");

    run(&scratch, &gh, &[]);

    let calls = gh.calls();
    assert_ne!(calls, "");
    for line in calls.lines() {
        assert!(
            line.contains("repos/owner/name"),
            "an unexpected call: {line}"
        );
    }
}

fn git(directory: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git to run");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_repository_is_taken_from_the_origin_remote_as_https_or_ssh() {
    for url in [
        "https://github.com/owner/name.git",
        "https://github.com/owner/name",
        "git@github.com:owner/name.git",
        "ssh://git@github.com/owner/name.git",
    ] {
        let scratch = repository("bs-remote");
        git(&scratch.0, &["init", "-q"]);
        git(&scratch.0, &["remote", "add", "origin", url]);
        let gh = compliant_github("bs-remote-gh");
        let path = format!(
            "{}:{}",
            gh.bin().display(),
            std::env::var("PATH").unwrap_or_default()
        );

        let output = heph_with_env(
            &scratch.0,
            &["baseline", "check", "--settings"],
            &[
                ("PATH", &path),
                ("HEPH_TEST_GH_DIR", &gh.scratch.0.to_string_lossy()),
            ],
        );

        assert_eq!(
            output.status.code(),
            Some(0),
            "{url}: stderr: {}",
            stderr(&output)
        );
        assert!(
            gh.calls().contains("repos/owner/name"),
            "{url}: {}",
            gh.calls()
        );
    }
}

#[test]
fn without_a_repo_and_without_a_github_remote_the_check_could_not_be_made() {
    for remote in [None, Some("https://example.org/owner/name.git")] {
        let scratch = repository("bs-no-remote");
        git(&scratch.0, &["init", "-q"]);
        if let Some(url) = remote {
            git(&scratch.0, &["remote", "add", "origin", url]);
        }
        let gh = compliant_github("bs-no-remote-gh");
        let path = format!(
            "{}:{}",
            gh.bin().display(),
            std::env::var("PATH").unwrap_or_default()
        );

        let output = heph_with_env(
            &scratch.0,
            &["baseline", "check", "--settings"],
            &[
                ("PATH", &path),
                ("HEPH_TEST_GH_DIR", &gh.scratch.0.to_string_lossy()),
            ],
        );

        assert_eq!(
            output.status.code(),
            Some(3),
            "{remote:?}: stderr: {}",
            stderr(&output)
        );
        assert_eq!(
            gh.calls(),
            "",
            "{remote:?}: GitHub should not have been contacted"
        );
    }
}

#[test]
fn without_settings_github_is_never_contacted() {
    let scratch = repository("bs-offline");
    let gh = compliant_github("bs-offline-gh");
    let path = format!(
        "{}:{}",
        gh.bin().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = heph_with_env(
        &scratch.0,
        &["baseline", "check", "--repo", REPO],
        &[
            ("PATH", &path),
            ("HEPH_TEST_GH_DIR", &gh.scratch.0.to_string_lossy()),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(gh.calls(), "");
}

#[test]
fn nothing_is_asked_of_github_but_reads() {
    let scratch = repository("bs-reads");
    let gh = compliant_github("bs-reads-gh");

    run(&scratch, &gh, &[]);

    for line in gh.calls().lines() {
        assert!(
            line.starts_with("api repos/") && !line.contains("--method") && !line.contains(" -X"),
            "a call that is not a plain read: {line}"
        );
    }
}
