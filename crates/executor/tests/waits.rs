//! Every wait that can outlast the control plane's silence limit writes heartbeats while it
//! lasts, not only the agent's and the gates': the bootstrap steps, the clone and checkout, the
//! push and the call that opens the pull request. Each has a stand-in that takes several
//! heartbeat intervals.

mod common;

use std::os::unix::fs::PermissionsExt as _;

use common::{
    IDENTITY, Ran, Rig, Scratch, agent_script, finished, kinds, manifest, rig, run_with,
    run_with_env, transcript, workload,
};
use contracts::v1alpha1::Event;
use contracts::v1alpha1::finished::Outcome;
use serde_json::json;

const SLEEP: &str = "4";

fn executable(path: &std::path::Path, script: &str) {
    std::fs::write(path, script).expect("a script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("to chmod");
}

/// A `git` that sleeps when it is run with `subcommand` among its arguments, and is the real
/// one otherwise. Returns the PATH that finds it first.
fn slow_git(scratch: &Scratch, subcommand: &str) -> String {
    let real = String::from_utf8(
        std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .expect("sh")
            .stdout,
    )
    .expect("UTF-8")
    .trim()
    .to_owned();
    let bin = scratch.path("bin");
    std::fs::create_dir_all(&bin).expect("a directory");
    executable(
        &bin.join("git"),
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = {subcommand} ] && sleep {SLEEP}; done\nexec {real} \"$@\"\n"
        ),
    );
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn position(events: &[Event], name: &str) -> usize {
    kinds(events)
        .iter()
        .position(|k| *k == name)
        .unwrap_or_else(|| panic!("no {name}: {:?}", kinds(events)))
}

fn beats_in(events: &[Event], from: usize, to: usize) -> usize {
    kinds(events)[from..to]
        .iter()
        .filter(|k| **k == "heartbeat")
        .count()
}

fn gapless(events: &[Event]) {
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, index as u64 + 1);
    }
}

#[test]
fn heartbeats_are_written_while_a_bootstrap_step_runs() {
    let scratch = Scratch::new("wait-bootstrap");
    let text = manifest(&[(&["sh", "-c", "sleep 4"], None, 30)]);
    let (_, commit) = common::source(&scratch, &text);

    let ran = run_with(
        scratch,
        &workload(&commit),
        Some(&transcript()),
        &["--heartbeat-seconds", "1"],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let ready = position(&events, "workspace_ready");
    let agent = position(&events, "agent_message");
    let beats = beats_in(&events, ready, agent);
    assert!(beats >= 2, "heartbeats during the bootstrap step: {beats}");
    gapless(&events);
}

#[test]
fn a_bootstrap_step_that_hangs_is_still_ended_by_its_own_timeout_while_heartbeats_continue() {
    let scratch = Scratch::new("wait-bootstrap-timeout");
    let text = manifest(&[(&["sh", "-c", "sleep 60"], None, 3)]);
    let (_, commit) = common::source(&scratch, &text);
    let started = std::time::Instant::now();

    let ran = run_with(
        scratch,
        &workload(&commit),
        Some(&transcript()),
        &["--heartbeat-seconds", "1"],
    );

    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Failed);
    assert!(finished(&events).1.contains("timed out"), "{events:?}");
    let ready = position(&events, "workspace_ready");
    assert!(beats_in(&events, ready, events.len()) >= 1);
}

fn beats_before_workspace_ready(name: &str, subcommand: &str) {
    let scratch = Scratch::new(name);
    let (_, commit) = common::source(&scratch, &manifest(&[]));
    let path = slow_git(&scratch, subcommand);

    let ran = run_with_env(
        scratch,
        &workload(&commit),
        Some(&transcript()),
        &["--heartbeat-seconds", "1"],
        &[("PATH", &path)],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let beats = beats_in(&events, 0, position(&events, "workspace_ready"));
    assert!(
        beats >= 2,
        "heartbeats between started and workspace_ready while git {subcommand} ran: {beats}"
    );
    gapless(&events);
}

#[test]
fn heartbeats_are_written_while_the_clone_runs() {
    beats_before_workspace_ready("wait-clone", "clone");
}

#[test]
fn heartbeats_are_written_while_the_checkout_runs() {
    beats_before_workspace_ready("wait-checkout", "checkout");
}

/// A delivering run whose `git` sleeps on `subcommand` and whose `gh` sleeps `gh_seconds`.
fn delivering(name: &str, subcommand: &str, gh_seconds: u32) -> Ran {
    let Rig {
        scratch, commit, ..
    } = rig(name, &manifest(&[]));
    delivering_workload(scratch, &workload(&commit), subcommand, gh_seconds)
}

fn delivering_workload(
    scratch: Scratch,
    value: &serde_json::Value,
    subcommand: &str,
    gh_seconds: u32,
) -> Ran {
    let path = slow_git(&scratch, subcommand);
    let gh = scratch.path("gh");
    executable(
        &gh,
        &format!(
            "#!/bin/sh\nsleep {gh_seconds}\necho https://github.com/erinversfeldcodes/gazebolang/pull/77\n"
        ),
    );
    let script = agent_script("");
    let agent = json!(["sh", "-c", script]).to_string();
    let gh = gh.display().to_string();
    let mut environment: Vec<(&str, &str)> = IDENTITY.to_vec();
    environment.push(("PATH", &path));
    run_with_env(
        scratch,
        value,
        None,
        &[
            "--agent-command",
            &agent,
            "--gh",
            &gh,
            "--deliver",
            "--heartbeat-seconds",
            "1",
        ],
        &environment,
    )
}

fn beats_during_delivery(ran: &Ran, what: &str) {
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Completed, "{}", ran.stderr());
    let last_agent = kinds(&events)
        .iter()
        .rposition(|k| *k == "agent_message")
        .expect("the agent spoke");
    let beats = beats_in(
        &events,
        last_agent,
        position(&events, "pull_request_opened"),
    );
    assert!(beats >= 2, "heartbeats while {what} ran: {beats}");
    gapless(&events);
}

#[test]
fn heartbeats_are_written_while_the_push_runs() {
    beats_during_delivery(&delivering("wait-push", "push", 0), "the push");
}

#[test]
fn heartbeats_are_written_while_the_pull_request_is_opened() {
    beats_during_delivery(
        &delivering("wait-open", "never-a-subcommand", 4),
        "the call that opens the pull request",
    );
}

#[test]
fn heartbeats_are_written_while_the_base_branch_is_fetched() {
    beats_during_delivery(
        &delivering("wait-fetch", "fetch", 0),
        "the fetch of the base branch",
    );
}

#[test]
fn heartbeats_are_written_while_a_revisions_pull_request_is_updated() {
    let Rig {
        scratch, source, ..
    } = rig("wait-update", &manifest(&[]));
    let branch = "heph/12-an-earlier-title-of-the-ticket";
    common::git(&source, &["switch", "-q", "-c", branch]);
    scratch.write("source/web/first.txt", "the first attempt\n");
    common::git(&source, &["add", "."]);
    common::git(&source, &["commit", "-q", "-m", "fix: the first attempt"]);
    let head = common::git(&source, &["rev-parse", "HEAD"]);
    common::git(&source, &["switch", "-q", "main"]);
    let mut value = workload(&head);
    value["revision"] = json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": "https://github.com/erinversfeldcodes/gazebolang/pull/52",
        "branch": branch,
        "attempt": 1,
        "failures": [{"name": "test", "result": "failure", "excerpt": "it failed"}]
    });

    let ran = delivering_workload(scratch, &value, "never-a-subcommand", 4);

    beats_during_delivery(&ran, "the update of the pull request");
}

#[test]
fn heartbeats_are_written_while_the_push_is_checked_before_the_agent_starts() {
    let ran = delivering("wait-push-check", "--dry-run", 0);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let beats = beats_in(&events, 0, position(&events, "workspace_ready"));
    assert!(
        beats >= 2,
        "heartbeats between started and workspace_ready while the dry-run push ran: {beats}"
    );
    gapless(&events);
}
