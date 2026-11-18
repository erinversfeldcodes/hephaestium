//! Delivery: what the executor does with a workload that has passed its gates.
//! It commits the agent's work, pushes a branch and opens the pull request, and
//! it is the only step that holds the token that can.

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use common::{Ran, Scratch, finished, git, kinds, manifest, run_with_env, source, workload};
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use serde_json::json;

const BRANCH: &str = "heph/12-make-the-reset-spec-wait-for-hydration";
const SUBJECT: &str = "fix: make the reset spec wait for hydration";
const MESSAGE: &str = "fix: make the reset spec wait for hydration\n\nThe spec clicked before the page was interactive.\n";

const IDENTITY: [(&str, &str); 5] = [
    ("HEPH_GITHUB_TOKEN", "a-github-token"),
    ("GIT_AUTHOR_NAME", "A Person"),
    ("GIT_AUTHOR_EMAIL", "person@example.invalid"),
    ("GIT_COMMITTER_NAME", "A Person"),
    ("GIT_COMMITTER_EMAIL", "person@example.invalid"),
];

/// A `gh` that records how it was called and prints a pull request's address.
fn stand_in_gh(scratch: &Scratch, exit: i32) -> PathBuf {
    let out = scratch.path("seen");
    std::fs::create_dir_all(&out).expect("a directory");
    let script = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$@\" > {out}/gh-args.txt\n\
         printf '%s\\n' \"${{GH_TOKEN:-none}}\" > {out}/gh-token.txt\n\
         prev=''\n\
         for a in \"$@\"; do if [ \"$prev\" = '--body-file' ]; then cp \"$a\" {out}/gh-body.txt; fi; prev=\"$a\"; done\n\
         [ {exit} -eq 0 ] && echo https://github.com/erinversfeldcodes/gazebolang/pull/77\n\
         exit {exit}\n",
        out = out.display(),
    );
    let gh = scratch.path("gh");
    std::fs::write(&gh, script).expect("a script");
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).expect("to chmod");
    gh
}

fn agent_script(extra: &str) -> String {
    // printf turns the \n in the message into newlines, and the message has no %.
    format!(
        "mkdir -p web; echo changed > web/changed.txt; printf '{}' > .heph-commit-message; {extra} echo '{}'",
        MESSAGE.replace('\n', "\\n"),
        json!({"agentMessage": {"text": "done"}})
    )
}

struct Rig {
    scratch: Scratch,
    commit: String,
    source: PathBuf,
}

fn rig(name: &str, manifest_text: &str) -> Rig {
    let scratch = Scratch::new(name);
    let (source_dir, commit) = source(&scratch, manifest_text);
    Rig {
        scratch,
        commit,
        source: source_dir,
    }
}

fn run(
    rig: Rig,
    script: &str,
    deliver: bool,
    gh_exit: i32,
    env: &[(&str, &str)],
) -> (Ran, PathBuf, String) {
    let gh = stand_in_gh(&rig.scratch, gh_exit);
    let mut arguments = vec![
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
    ];
    if deliver {
        arguments.push("--deliver".to_owned());
    }
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let ran = run_with_env(rig.scratch, &workload(&rig.commit), None, &refs, env);
    (ran, rig.source, rig.commit)
}

fn has_branch(source: &Path) -> bool {
    std::process::Command::new("git")
        .current_dir(source)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{BRANCH}"),
        ])
        .output()
        .expect("git")
        .status
        .success()
}

fn gh_was_called(ran: &Ran) -> bool {
    ran.scratch.path("seen/gh-args.txt").exists()
}

#[test]
fn a_completed_workload_becomes_a_commit_a_branch_and_a_pull_request() {
    let (ran, source, base) = run(
        rig("deliver-ok", &manifest(&[])),
        &agent_script(""),
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
    let scratch_for_paths = Scratch::new("deliver-paths-unused");
    drop(scratch_for_paths);
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
fn a_workload_that_changed_nothing_delivers_nothing() {
    let script = format!(
        "printf '%s\\n' '{SUBJECT}' > .heph-commit-message; echo '{}'",
        json!({"agentMessage": {"text": "nothing to do"}})
    );
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
