//! `heph workload TICKET`: building the workload for a ticket from what GitHub
//! says about it, and printing it.
//!
//! GitHub is stood in for by the `gh` the baseline tests use: it answers from
//! recorded responses shaped like GitHub's real ones. That proves the logic
//! against those shapes, not that GitHub answers that way; the command is also
//! run against a real repository.

mod common;

use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use common::github::Gh;
use common::{Scratch, heph_with_env, stderr, stdout};
use contracts::v1alpha1::{AgentRuntime, CostUnit, Workload, agent_runtime};
use serde_json::json;

const REPO: &str = "acme/widgets";
const TIP: &str = "0123456789abcdef0123456789abcdef01234567";

/// A stand-in GitHub that knows a repository, its default branch and tip, a
/// signed-in user, and an open issue 12.
fn github(name: &str) -> Gh {
    let gh = Gh::new(name);
    gh.respond("repos/acme/widgets", &json!({ "default_branch": "main" }));
    gh.respond("repos/acme/widgets/commits/main", &json!({ "sha": TIP }));
    gh.respond("user", &json!({ "login": "a-maintainer" }));
    gh.respond(
        "repos/acme/widgets/issues/12",
        &json!({
            "number": 12,
            "title": "Make the reset spec wait for hydration",
            "body": "The spec clicks Sign in before the page is interactive.",
            "html_url": "https://github.com/acme/widgets/issues/12",
            "state": "open"
        }),
    );
    gh
}

fn run(directory: &Path, gh: &Gh, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        gh.bin().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut all = vec!["workload"];
    all.extend_from_slice(args);
    heph_with_env(
        directory,
        &all,
        &[
            ("PATH", path.as_str()),
            ("HEPH_TEST_GH_DIR", &gh.scratch.0.to_string_lossy()),
        ],
    )
}

fn workload_of(output: &Output) -> Workload {
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|e| panic!("a workload the contract accepts ({e}): {}", stderr(output)))
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned()
}

#[test]
fn a_ticket_becomes_a_workload_the_contract_accepts() {
    let scratch = Scratch::new("wl-ok");
    let gh = github("wl-ok-gh");

    let output = run(&scratch.0, &gh, &["12", "--repo", REPO]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let workload = workload_of(&output);
    let ticket = workload.ticket.as_ref().expect("a ticket");
    assert_eq!(ticket.number, 12);
    assert_eq!(ticket.title, "Make the reset spec wait for hydration");
    assert_eq!(
        ticket.body,
        "The spec clicks Sign in before the page is interactive."
    );
    assert_eq!(ticket.url, "https://github.com/acme/widgets/issues/12");

    let repository = workload.repository.as_ref().expect("a repository");
    assert_eq!(repository.owner, "acme");
    assert_eq!(repository.name, "widgets");
    assert_eq!(repository.base_branch, "main");
    assert_eq!(
        repository.base_commit, TIP,
        "the tip of the default branch on GitHub"
    );

    let runtime: &AgentRuntime = workload.runtime.as_ref().expect("a runtime");
    assert_eq!(runtime.kind, agent_runtime::Kind::ClaudeCode as i32);
    assert_eq!(
        runtime.provider,
        agent_runtime::Provider::AnthropicSubscription as i32
    );
    assert_eq!(runtime.model, "sonnet");

    let limits = workload.limits.as_ref().expect("limits");
    assert_eq!(limits.timeout.as_ref().expect("a timeout").seconds, 1800);
    let budget = limits.budget.as_ref().expect("a budget");
    assert_eq!(budget.unit, CostUnit::Usd as i32);
    assert!((budget.amount - 5.0).abs() < f64::EPSILON);

    // Running the command is the approval, by the signed-in user, now.
    let approval = workload.approval.as_ref().expect("an approval");
    assert_eq!(approval.approved_by, "a-maintainer");
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("a clock")
            .as_secs(),
    )
    .expect("seconds");
    let approved_at = approval.approved_at.as_ref().expect("a time").seconds;
    assert!(
        (now - approved_at).abs() < 120,
        "approved {approved_at}, now {now}"
    );

    // A fresh random id, in the shape of a version 4 UUID.
    let id = workload.id.as_bytes();
    assert_eq!(id.len(), 36, "{}", workload.id);
    assert!(
        [8, 13, 18, 23].iter().all(|i| id[*i] == b'-'),
        "{}",
        workload.id
    );
    assert_eq!(id[14], b'4', "{}", workload.id);
    assert!(
        matches!(id[19], b'8' | b'9' | b'a' | b'b'),
        "{}",
        workload.id
    );
    assert!(
        workload
            .id
            .bytes()
            .all(|b| b == b'-' || b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
}

#[test]
fn options_override_the_defaults_and_each_workload_has_a_fresh_id() {
    let scratch = Scratch::new("wl-options");
    let gh = github("wl-options-gh");

    let first = run(
        &scratch.0,
        &gh,
        &[
            "#12",
            "--repo",
            REPO,
            "--model",
            "opus",
            "--timeout-seconds",
            "600",
            "--budget-usd",
            "1.5",
        ],
    );
    let second = run(&scratch.0, &gh, &["12", "--repo", REPO]);

    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let workload = workload_of(&first);
    assert_eq!(workload.runtime.as_ref().expect("a runtime").model, "opus");
    let limits = workload.limits.as_ref().expect("limits");
    assert_eq!(limits.timeout.as_ref().expect("a timeout").seconds, 600);
    assert!((limits.budget.as_ref().expect("a budget").amount - 1.5).abs() < f64::EPSILON);
    assert_ne!(workload.id, workload_of(&second).id);
}

#[test]
fn the_repository_comes_from_the_checkouts_origin() {
    let checkout = Scratch::new("wl-origin");
    git(&checkout.0, &["init", "-q", "-b", "main"]);
    git(
        &checkout.0,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    );
    let gh = github("wl-origin-gh");

    let output = run(&checkout.0, &gh, &["12"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let repository = workload_of(&output).repository.expect("a repository");
    assert_eq!(
        (repository.owner.as_str(), repository.name.as_str()),
        ("acme", "widgets")
    );
}

#[test]
fn a_pull_request_a_closed_issue_and_a_missing_one_are_refused_with_a_reason() {
    for (name, endpoint_body, expect) in [
        (
            "pull-request",
            Some(
                json!({"number": 12, "title": "t", "body": "b", "html_url": "u", "state": "open", "pull_request": {"url": "x"}}),
            ),
            "pull request",
        ),
        (
            "closed",
            Some(
                json!({"number": 12, "title": "t", "body": "b", "html_url": "u", "state": "closed"}),
            ),
            "not open",
        ),
        ("missing", None, "no issue 12"),
    ] {
        let scratch = Scratch::new(&format!("wl-refused-{name}"));
        let gh = github(&format!("wl-refused-{name}-gh"));
        match endpoint_body {
            Some(body) => {
                gh.respond("repos/acme/widgets/issues/12", &body);
            }
            None => {
                std::fs::remove_file(gh.scratch.0.join(format!(
                    "{}.json",
                    Gh::file_for("repos/acme/widgets/issues/12")
                )))
                .expect("to remove");
            }
        }

        let output = run(&scratch.0, &gh, &["12", "--repo", REPO]);

        assert_eq!(output.status.code(), Some(3), "{name}: {}", stderr(&output));
        assert_eq!(stdout(&output), "", "{name}: something was printed");
        assert!(
            stderr(&output).contains(expect),
            "{name}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn a_checkout_at_another_commit_than_the_tip_is_warned_about_and_one_at_the_tip_is_not() {
    let checkout = Scratch::new("wl-warn");
    git(&checkout.0, &["init", "-q", "-b", "main"]);
    std::fs::write(checkout.0.join("a.txt"), "a").expect("a file");
    git(&checkout.0, &["add", "."]);
    git(&checkout.0, &["commit", "-q", "-m", "first"]);
    let head = git(&checkout.0, &["rev-parse", "HEAD"]);
    let gh = github("wl-warn-gh");

    let differs = run(
        &checkout.0,
        &gh,
        &[
            "12",
            "--repo",
            REPO,
            "--root",
            &checkout.0.to_string_lossy(),
        ],
    );
    assert_eq!(differs.status.code(), Some(0), "{}", stderr(&differs));
    assert!(
        stderr(&differs).contains("tip"),
        "no warning: {}",
        stderr(&differs)
    );
    assert!(
        stderr(&differs).contains(&head[..7]),
        "{}",
        stderr(&differs)
    );
    assert_eq!(
        workload_of(&differs)
            .repository
            .expect("a repository")
            .base_commit,
        TIP
    );

    gh.respond("repos/acme/widgets/commits/main", &json!({ "sha": head }));
    let same = run(
        &checkout.0,
        &gh,
        &[
            "12",
            "--repo",
            REPO,
            "--root",
            &checkout.0.to_string_lossy(),
        ],
    );
    assert_eq!(same.status.code(), Some(0), "{}", stderr(&same));
    assert!(
        !stderr(&same).contains("tip"),
        "a warning when there was nothing to warn about: {}",
        stderr(&same)
    );
}

#[test]
fn a_missing_body_is_an_empty_one() {
    let scratch = Scratch::new("wl-null-body");
    let gh = github("wl-null-body-gh");
    gh.respond(
        "repos/acme/widgets/issues/12",
        &json!({"number": 12, "title": "A ticket with no body", "body": null, "html_url": "https://github.com/acme/widgets/issues/12", "state": "open"}),
    );

    let output = run(&scratch.0, &gh, &["12", "--repo", REPO]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(workload_of(&output).ticket.expect("a ticket").body, "");
}

#[test]
fn a_bad_option_or_ticket_is_a_usage_error() {
    let scratch = Scratch::new("wl-usage");
    let gh = github("wl-usage-gh");
    for args in [
        vec!["12", "--repo", REPO, "--budget-usd", "0"],
        vec!["12", "--repo", REPO, "--budget-usd", "-1"],
        vec!["12", "--repo", REPO, "--budget-usd", "lots"],
        vec!["12", "--repo", REPO, "--timeout-seconds", "0"],
        vec!["twelve", "--repo", REPO],
        vec!["0", "--repo", REPO],
        vec!["-3", "--repo", REPO],
        vec!["--repo", REPO],
        vec!["12", "13", "--repo", REPO],
        vec!["12", "--repo", REPO, "--nonsense"],
    ] {
        let output = run(&scratch.0, &gh, &args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), "", "{args:?}");
    }
}

#[test]
fn gh_that_cannot_be_reached_is_a_reason_and_not_a_workload() {
    let scratch = Scratch::new("wl-gh-fails");
    let gh = github("wl-gh-fails-gh");
    gh.fail("repos/acme/widgets", "gh: HTTP 502 Bad Gateway");

    let output = run(&scratch.0, &gh, &["12", "--repo", REPO]);

    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("502"), "{}", stderr(&output));
}
