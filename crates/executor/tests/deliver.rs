//! Delivery: what the executor does with a workload that has passed its gates.
//! It commits the agent's work, pushes a branch and opens the pull request, and
//! it is the only step that holds the token that can.

mod common;

use common::{
    BRANCH, IDENTITY, MESSAGE, Rig, SUBJECT, Scratch, agent_script, finished, gh_was_called, git,
    has_branch, kinds, manifest, rig, run, source,
};
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use serde_json::json;

#[test]
fn a_completed_workload_becomes_a_commit_a_branch_and_a_pull_request() {
    let (ran, source, base) = run(
        rig("deliver-ok", &manifest(&[])),
        &agent_script("printf 'complete\\n' > .heph-resolution;"),
        true,
        0,
        &IDENTITY,
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let names = kinds(&events);
    assert_eq!(
        &names[names.len() - 2..],
        ["pull_request_opened", "finished"],
        "{names:?}"
    );
    assert_eq!(finished(&events).0, Outcome::Completed);
    let Some(Kind::PullRequestOpened(pr)) = events[events.len() - 2].kind.as_ref() else {
        panic!("{names:?}");
    };
    assert_eq!(
        pr.url,
        "https://github.com/erinversfeldcodes/gazebolang/pull/77"
    );
    assert_eq!(pr.number, 77);
    assert_eq!(pr.head_branch, BRANCH);
    assert_eq!(
        ran.record()
            .pull_request
            .as_ref()
            .expect("a pull request")
            .number,
        77
    );

    // The branch, from the base commit, with the agent's message and the
    // configured identity, and nothing the agent was not meant to leave.
    assert_eq!(git(&source, &["rev-parse", &format!("{BRANCH}^")]), base);
    assert_eq!(
        git(
            &source,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>", BRANCH]
        ),
        "A Person <person@example.invalid>|A Person <person@example.invalid>"
    );
    assert_eq!(
        git(&source, &["log", "-1", "--format=%B", BRANCH]).trim_end(),
        MESSAGE.trim_end()
    );
    let files = git(&source, &["show", "--name-only", "--format=", BRANCH]);
    assert_eq!(
        files.trim(),
        "web/changed.txt",
        "the commit holds the agent's change and only that"
    );

    // The pull request.
    let args =
        std::fs::read_to_string(ran.scratch.path("seen/gh-args.txt")).expect("gh was called");
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(&args[..2], ["pr", "create"]);
    let flag = |name: &str| -> &str {
        let at = args
            .iter()
            .position(|a| *a == name)
            .unwrap_or_else(|| panic!("no {name}: {args:?}"));
        args[at + 1]
    };
    assert_eq!(flag("--repo"), "erinversfeldcodes/gazebolang");
    assert_eq!(flag("--head"), BRANCH);
    assert_eq!(flag("--base"), "main");
    assert_eq!(flag("--title"), SUBJECT);
    let body = std::fs::read_to_string(ran.scratch.path("seen/gh-body.txt")).expect("a body");
    assert!(
        body.contains("The spec clicked before the page was interactive."),
        "{body}"
    );
    assert!(
        body.contains("erinversfeldcodes") && body.contains("2026-11-17"),
        "who approved, and when: {body}"
    );
    assert_eq!(body.trim_end().lines().last(), Some("Closes #12"), "{body}");
}

#[test]
fn the_token_reaches_the_pull_request_and_not_the_bootstrap_the_gates_or_the_agent() {
    let rig = {
        let scratch = Scratch::new("deliver-token");
        let out = scratch.path("seen");
        std::fs::create_dir_all(&out).expect("a directory");
        let text = manifest(&[(
            &[
                "sh",
                "-c",
                &format!("env > {}/bootstrap-env.txt", out.display()),
            ],
            None,
            30,
        )]) + &format!(
            "\n[gates.env]\nrun = [\"sh\", \"-c\", \"env > {}/gate-env.txt\"]\ntimeout_seconds = 30\n",
            out.display()
        );
        let (source_dir, commit) = source(&scratch, &text);
        Rig {
            scratch,
            commit,
            source: source_dir,
        }
    };
    let out = rig.scratch.path("seen");
    let script = agent_script(&format!("env > {}/agent-env.txt;", out.display()));

    let (ran, _, _) = run(rig, &script, true, 0, &IDENTITY);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    for file in ["bootstrap-env.txt", "gate-env.txt", "agent-env.txt"] {
        let env = std::fs::read_to_string(ran.scratch.path(&format!("seen/{file}"))).expect(file);
        assert!(
            !env.contains("a-github-token"),
            "{file} holds the token:\n{env}"
        );
        assert!(
            !env.contains("HEPH_GITHUB_TOKEN"),
            "{file} names the token variable:\n{env}"
        );
    }
    let token =
        std::fs::read_to_string(ran.scratch.path("seen/gh-token.txt")).expect("gh saw a token");
    assert_eq!(
        token.trim(),
        "a-github-token",
        "the pull request is opened with the token"
    );
}

#[test]
fn a_missing_or_not_conventional_message_delivers_nothing_and_says_why() {
    for (name, write) in [
        ("none", String::new()),
        (
            "plain",
            "printf 'Fixed stuff\\n' > .heph-commit-message;".to_owned(),
        ),
        ("empty", ": > .heph-commit-message;".to_owned()),
        (
            "bad-type",
            "printf 'wip: stuff\\n' > .heph-commit-message;".to_owned(),
        ),
        (
            "no-description",
            "printf 'fix: \\n\\nbody\\n' > .heph-commit-message;".to_owned(),
        ),
        (
            "bad-scope",
            "printf 'fix(Bad Scope): stuff\\n' > .heph-commit-message;".to_owned(),
        ),
    ] {
        let script = format!(
            "mkdir -p web; echo changed > web/changed.txt; {write} echo '{}'",
            json!({"agentMessage": {"text": "done"}})
        );
        let (ran, source, _) = run(
            rig(&format!("deliver-msg-{name}"), &manifest(&[])),
            &script,
            true,
            0,
            &IDENTITY,
        );

        let events = ran.events();
        let (outcome, reason) = finished(&events);
        assert_eq!(outcome, Outcome::Failed, "{name}: {reason}");
        assert!(reason.contains(".heph-commit-message"), "{name}: {reason}");
        assert!(!has_branch(&source), "{name}: a branch was pushed");
        assert!(!gh_was_called(&ran), "{name}: gh was called");
        assert!(!kinds(&events).contains(&"pull_request_opened"), "{name}");
    }
}

#[test]
fn a_workload_that_changed_nothing_and_said_nothing_delivers_nothing() {
    // With no account of why, there is nothing for a person to answer, so it fails.
    let script = format!("printf '%s\\n' '{SUBJECT}' > .heph-commit-message;");
    let (ran, source, _) = run(
        rig("deliver-nothing", &manifest(&[])),
        &script,
        true,
        0,
        &IDENTITY,
    );

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("changed nothing"), "{reason}");
    assert!(!has_branch(&source));
    assert!(!gh_was_called(&ran));
}

#[test]
fn a_failed_gate_delivers_nothing() {
    let text = manifest(&[])
        + "\n[gates.fails]\nrun = [\"sh\", \"-c\", \"exit 1\"]\ntimeout_seconds = 30\n";
    let (ran, source, _) = run(
        rig("deliver-gate", &text),
        &agent_script(""),
        true,
        0,
        &IDENTITY,
    );

    assert_eq!(finished(&ran.events()).0, Outcome::Failed);
    assert!(!has_branch(&source));
    assert!(!gh_was_called(&ran));
}

#[test]
fn a_push_that_is_rejected_fails_the_workload_and_no_pull_request_is_opened() {
    let rig = rig("deliver-rejected", &manifest(&[]));
    // The branch already exists with history of its own, so a push of the
    // workload's commit is not a fast-forward.
    git(&rig.source, &["switch", "-q", "-c", BRANCH]);
    std::fs::write(rig.source.join("other.txt"), "someone else's\n").expect("a file");
    git(&rig.source, &["add", "."]);
    git(&rig.source, &["commit", "-q", "-m", "other"]);
    git(&rig.source, &["switch", "-q", "main"]);

    let (ran, _, _) = run(rig, &agent_script(""), true, 0, &IDENTITY);

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("push"), "{reason}");
    assert!(!gh_was_called(&ran));
}

#[test]
fn delivery_is_announced_once_right_before_the_push() {
    let (ran, _, _) = run(
        rig("deliver-began", &manifest(&[])),
        &agent_script("printf 'complete\\n' > .heph-resolution;"),
        true,
        0,
        &IDENTITY,
    );

    let names = kinds(&ran.events());
    assert_eq!(
        &names[names.len() - 3..],
        ["delivery_began", "pull_request_opened", "finished"],
        "{names:?}"
    );
    assert_eq!(
        names
            .iter()
            .filter(|name| **name == "delivery_began")
            .count(),
        1,
        "{names:?}"
    );
}

#[test]
fn delivery_has_been_announced_by_the_time_the_push_arrives() {
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::Stdio;

    let rig = rig("deliver-began-before-push", &manifest(&[]));
    let live = rig.scratch.path("events-live.jsonl");
    let at_push = rig.scratch.path("events-at-push.jsonl");
    // The receiving repository keeps a copy of what the executor had said when the push arrived.
    let hook = rig.source.join(".git/hooks/pre-receive");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ncp {} {}\n", live.display(), at_push.display()),
    )
    .expect("a hook");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let gh = common::stand_in_gh(&rig.scratch, 0);
    let agent = agent_script("printf 'complete\\n' > .heph-resolution;");
    let mut command = common::command_for(
        &rig.scratch,
        &common::workload(&rig.commit),
        None,
        &[
            "--agent-command",
            &json!(["sh", "-c", agent]).to_string(),
            "--gh",
            &gh.display().to_string(),
            "--deliver",
        ],
        &IDENTITY,
    );
    let status = command
        .stdout(Stdio::from(std::fs::File::create(&live).expect("a file")))
        .stderr(Stdio::null())
        .status()
        .expect("the executor to run");
    assert!(status.success());

    let said = std::fs::read_to_string(&at_push).expect("the push reached the repository");
    assert!(
        said.contains("deliveryBegan"),
        "the push arrived before delivery was announced: {said}"
    );
}

#[test]
fn delivery_is_announced_even_when_the_push_is_then_rejected() {
    // The push may have reached the host before it failed, so what was said before it stands.
    let rig = rig("deliver-began-rejected", &manifest(&[]));
    git(&rig.source, &["switch", "-q", "-c", BRANCH]);
    std::fs::write(rig.source.join("other.txt"), "someone else's\n").expect("a file");
    git(&rig.source, &["add", "."]);
    git(&rig.source, &["commit", "-q", "-m", "other"]);
    git(&rig.source, &["switch", "-q", "main"]);

    let (ran, _, _) = run(rig, &agent_script(""), true, 0, &IDENTITY);

    let names = kinds(&ran.events());
    assert_eq!(
        &names[names.len() - 2..],
        ["delivery_began", "finished"],
        "{names:?}"
    );
}

#[test]
fn a_run_that_does_not_deliver_never_announces_delivery() {
    let (ran, _, _) = run(
        rig("deliver-began-off", &manifest(&[])),
        &agent_script(""),
        false,
        0,
        &IDENTITY,
    );

    let names = kinds(&ran.events());
    assert!(!names.contains(&"delivery_began"), "{names:?}");
}

#[test]
fn a_run_that_fails_before_the_push_never_announces_delivery() {
    let text = manifest(&[])
        + "\n[gates.fails]\nrun = [\"sh\", \"-c\", \"exit 1\"]\ntimeout_seconds = 30\n";
    let (ran, _, _) = run(
        rig("deliver-began-gate", &text),
        &agent_script(""),
        true,
        0,
        &IDENTITY,
    );

    let names = kinds(&ran.events());
    assert!(!names.contains(&"delivery_began"), "{names:?}");
}

#[test]
fn a_pull_request_that_cannot_be_opened_fails_the_workload() {
    let (ran, source, _) = run(
        rig("deliver-gh-fails", &manifest(&[])),
        &agent_script(""),
        true,
        1,
        &IDENTITY,
    );

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("pull request"), "{reason}");
    assert!(has_branch(&source), "the branch was pushed, and stays");
}

#[test]
fn without_deliver_nothing_is_pushed() {
    let (ran, source, _) = run(
        rig("deliver-off", &manifest(&[])),
        &agent_script(""),
        false,
        0,
        &IDENTITY,
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert!(!has_branch(&source));
    assert!(!gh_was_called(&ran));
    assert!(ran.record().pull_request.is_none());
}

#[test]
fn deliver_with_no_token_is_refused_before_anything_is_created() {
    let identity_only: Vec<(&str, &str)> = IDENTITY
        .iter()
        .copied()
        .filter(|(k, _)| *k != "HEPH_GITHUB_TOKEN")
        .collect();
    let (ran, _, _) = run(
        rig("deliver-no-token", &manifest(&[])),
        &agent_script(""),
        true,
        0,
        &identity_only,
    );

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert!(
        ran.stderr().contains("HEPH_GITHUB_TOKEN"),
        "{}",
        ran.stderr()
    );
    assert_eq!(ran.output.stdout, b"");
    assert!(!ran.scratch.path("work").exists());
}
