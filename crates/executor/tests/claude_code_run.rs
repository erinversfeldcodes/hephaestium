//! The Claude Code adapter through the executor, with a stand-in `claude` that
//! prints a recorded run and records how it was started.

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use common::{Scratch, finished, kinds, manifest, run_with_env, source, workload};
use contracts::v1alpha1::finished::Outcome;
use serde_json::json;

const RECORDED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/claude-code/read-file.jsonl"
);

/// A directory holding a `claude` that records its arguments and environment in
/// `out` and prints the recorded run.
fn stand_in(scratch: &Scratch) -> PathBuf {
    let bin = scratch.path("bin");
    std::fs::create_dir_all(&bin).expect("a directory");
    let out = scratch.path("seen");
    std::fs::create_dir_all(&out).expect("a directory");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > {out}/args.txt\nenv > {out}/env.txt\ncat {recorded}\n",
        out = out.display(),
        recorded = RECORDED
    );
    let claude = bin.join("claude");
    std::fs::write(&claude, script).expect("a script");
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).expect("to chmod");
    bin
}

#[test]
fn a_claude_code_workload_runs_the_adapter_with_an_empty_config_dir_the_token_and_no_other_variable()
 {
    let scratch = Scratch::new("cc-run");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let bin = stand_in(&scratch);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").expect("a PATH")
    );

    let ran = run_with_env(
        scratch,
        &workload(&commit),
        None,
        &[],
        &[
            ("PATH", &path),
            ("CLAUDE_CODE_OAUTH_TOKEN", "a-token"),
            ("HEPH_TEST_SECRET", "must-not-reach-the-agent"),
            ("ANTHROPIC_API_KEY", "must-not-reach-the-agent"),
        ],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    assert_eq!(
        kinds(&events),
        [
            "started",
            "workspace_ready",
            "tool_call",
            "tool_result",
            "agent_message",
            "cost",
            "cost",
            "finished"
        ]
    );
    assert_eq!(finished(&events).0, Outcome::Completed);

    let env = std::fs::read_to_string(ran.scratch.path("seen/env.txt")).expect("the environment");
    assert!(env.contains("CLAUDE_CODE_OAUTH_TOKEN=a-token"), "{env}");
    assert!(
        env.lines()
            .any(|l| l.starts_with("CLAUDE_CONFIG_DIR=") && l.ends_with(".claude-config")),
        "{env}"
    );
    for forbidden in ["HEPH_TEST_SECRET", "ANTHROPIC_API_KEY", "HOME="] {
        assert!(
            !env.contains(forbidden),
            "{forbidden} reached the agent:\n{env}"
        );
    }

    let id = workload(&commit)["id"].as_str().expect("an id").to_owned();
    assert!(
        ran.scratch
            .path(&format!("work/{id}.claude-config/settings.json"))
            .is_file(),
        "the run's own configuration directory was not made before the agent started"
    );

    let args = std::fs::read_to_string(ran.scratch.path("seen/args.txt")).expect("the arguments");
    assert!(
        args.contains("Make the reset spec wait for hydration"),
        "{args}"
    );

    let record = ran.record();
    assert_eq!(record.costs.len(), 2, "a dollar total and a token total");
    assert_eq!(record.outcome, Outcome::Completed as i32);
}

#[test]
fn a_workload_for_a_runtime_with_no_adapter_is_refused_and_nothing_is_created() {
    let scratch = Scratch::new("cc-other-runtime");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let mut other = workload(&commit);
    other["runtime"]["kind"] = json!("KIND_PI");

    let ran = run_with_env(scratch, &other, None, &[], &[]);

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert!(
        ran.stderr().to_lowercase().contains("pi"),
        "{}",
        ran.stderr()
    );
    assert_eq!(ran.output.stdout, b"");
    assert!(!ran.scratch.path("work").exists());
}
