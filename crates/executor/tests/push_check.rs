//! A run that will push finds out before any agent starts whether it can, and when a push is
//! refused the host's own message, with every held secret taken out, is the reason the run
//! ends failed with.

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use common::{
    IDENTITY, Ran, agent_script, finished, kinds, manifest, rig, run_with_env, stand_in_gh,
    workload,
};
use contracts::v1alpha1::finished::Outcome;
use serde_json::json;

const TOKEN: &str = "a-github-token";

/// The first `git` on the executor's path that is not a stand-in.
fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("a PATH"))
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("a git")
}

/// What the stand-in git refuses.
#[derive(Clone, Copy)]
enum Refuse {
    Nothing,
    /// The dry-run push, as a host that does not let the token push does.
    TheDryRun,
    /// Only the real push.
    TheRealPush,
}

/// Run a delivering workload under a `git` that logs every push it is asked for, passes
/// everything else to the real one, and refuses a push as a host does when the token cannot
/// write. Returns the run and the push log, and where the agent leaves a mark when it starts.
fn run(name: &str, refuse: Refuse, deliver: bool, with_token: bool) -> (Ran, String, PathBuf) {
    let rig = rig(name, &manifest(&[]));
    let bin = rig.scratch.path("bin");
    std::fs::create_dir_all(&bin).expect("a directory");
    let log = rig.scratch.path("pushes.txt");
    let mode = match refuse {
        Refuse::Nothing => "none",
        Refuse::TheDryRun => "dry",
        Refuse::TheRealPush => "real",
    };
    let script = format!(
        "#!/bin/sh\n\
         for a in \"$@\"; do if [ \"$a\" = push ]; then\n\
           pushing=1\n\
           for b in \"$@\"; do [ \"$b\" = --dry-run ] && dry=1; done\n\
           if [ -n \"$dry\" ]; then echo \"dry $*\" >> \"$GIT_STAND_IN_LOG\"; else echo \"real $*\" >> \"$GIT_STAND_IN_LOG\"; fi\n\
           if {{ [ \"$GIT_STAND_IN_REFUSE\" = dry ] && [ -n \"$dry\" ]; }} || {{ [ \"$GIT_STAND_IN_REFUSE\" = real ] && [ -z \"$dry\" ]; }}; then\n\
             echo \"remote: Permission to erinversfeldcodes/gazebolang.git denied to the-bot.\" >&2\n\
             echo \"fatal: unable to access 'https://x-access-token:$GH_TOKEN@github.com/erinversfeldcodes/gazebolang.git/': The requested URL returned error: 403\" >&2\n\
             exit 128\n\
           fi\n\
         fi; done\n\
         exec {real} \"$@\"\n",
        real = real_git().display(),
    );
    let stand_in = bin.join("git");
    std::fs::write(&stand_in, script).expect("a script");
    std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let marker = rig.scratch.path("agent-ran");
    let agent = agent_script(&format!("touch {};", marker.display()));
    let gh = stand_in_gh(&rig.scratch, 0);
    let mut arguments = vec![
        "--agent-command".to_owned(),
        json!(["sh", "-c", agent]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
    ];
    if deliver {
        arguments.push("--deliver".to_owned());
    }
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").expect("a PATH")
    );
    let log_path = log.display().to_string();
    let mut environment: Vec<(&str, &str)> = IDENTITY
        .iter()
        .copied()
        .filter(|(name, _)| with_token || *name != "HEPH_GITHUB_TOKEN")
        .collect();
    environment.extend([
        ("PATH", path.as_str()),
        ("GIT_STAND_IN_LOG", log_path.as_str()),
        ("GIT_STAND_IN_REFUSE", mode),
    ]);
    let ran = run_with_env(
        rig.scratch,
        &workload(&rig.commit),
        None,
        &refs,
        &environment,
    );
    let pushes = std::fs::read_to_string(&log).unwrap_or_default();
    (ran, pushes, marker)
}

#[test]
fn a_push_the_host_refuses_before_the_agent_starts_ends_the_run_failed_at_once() {
    let (ran, pushes, marker) = run("check-refused", Refuse::TheDryRun, true, true);

    let events = ran.events();
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(!marker.exists(), "the agent was started");
    let names = kinds(&events);
    assert!(!names.contains(&"cost"), "something was spent: {names:?}");
    assert!(
        !names.contains(&"step_started"),
        "an agent step began: {names:?}"
    );
    assert!(
        reason.contains("Permission to erinversfeldcodes/gazebolang.git denied")
            && reason.contains("403"),
        "the host's message is the reason: {reason}"
    );
    assert!(
        reason.contains("write access"),
        "what to check is said: {reason}"
    );
    assert!(
        !reason.contains(TOKEN),
        "the reason holds the token: {reason}"
    );
    assert!(!ran.record().reason.contains(TOKEN));
    assert_eq!(pushes.lines().count(), 1, "{pushes}");
    assert!(
        pushes.starts_with("dry ") && pushes.contains("--dry-run"),
        "{pushes}"
    );
    assert!(
        ran.stderr()
            .contains("Permission to erinversfeldcodes/gazebolang.git denied"),
        "the host's message still reaches the executor's log"
    );
}

#[test]
fn a_push_that_is_refused_after_the_dry_run_passed_says_what_the_host_said() {
    let (ran, pushes, marker) = run("check-real-refused", Refuse::TheRealPush, true, true);

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(marker.exists(), "the agent did not run");
    assert!(
        reason.contains("exited with code 128")
            && reason.contains("Permission to erinversfeldcodes/gazebolang.git denied")
            && reason.contains("403"),
        "{reason}"
    );
    assert!(
        !reason.contains(TOKEN),
        "the reason holds the token: {reason}"
    );
    let lines: Vec<&str> = pushes.lines().collect();
    assert_eq!(lines.len(), 2, "{pushes}");
    assert!(
        lines[0].starts_with("dry ") && lines[1].starts_with("real "),
        "{pushes}"
    );
}

#[test]
fn a_token_that_can_push_runs_as_before_and_the_check_leaves_nothing_in_the_repository() {
    let (ran, pushes, marker) = run("check-passes", Refuse::Nothing, true, true);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    assert!(marker.exists());
    let lines: Vec<&str> = pushes.lines().collect();
    assert_eq!(lines.len(), 2, "{pushes}");
    assert!(
        lines[0].starts_with("dry ") && lines[1].starts_with("real "),
        "{pushes}"
    );
    let branches = common::git(&ran.scratch.path("source"), &["branch", "--list"]);
    assert!(
        !branches.contains("push-check"),
        "the dry run created a branch: {branches}"
    );
}

#[test]
fn a_run_that_is_not_delivering_checks_nothing() {
    for (name, deliver, with_token) in [("no-deliver", false, true), ("no-token", false, false)] {
        let (ran, pushes, marker) = run(
            &format!("check-{name}"),
            Refuse::TheDryRun,
            deliver,
            with_token,
        );

        assert!(pushes.is_empty(), "{name}: pushed {pushes}");
        assert!(marker.exists(), "{name}: the agent did not run");
        assert_eq!(finished(&ran.events()).0, Outcome::Completed, "{name}");
    }
}
