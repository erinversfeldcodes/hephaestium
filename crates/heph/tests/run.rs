//! `heph run TICKET`: building the workload, running the executor on it, showing
//! what happens and ending with the pull request.
//!
//! GitHub is stood in for by the `gh` the baseline tests use, and the executor
//! by a program that records how it was started and prints a recorded run, so
//! this proves what `heph run` does and not what the executor does. The two were
//! run together against a real repository by hand.

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::github::Gh;
use common::{Scratch, heph_with_env, stderr, stdout};
use serde_json::json;

const PR_URL: &str = "https://github.com/acme/widgets/pull/52";

/// Everything one test needs: a scratch directory that is the "home", the
/// stand-in GitHub, the stand-in executor and its canned output.
struct Rig {
    home: Scratch,
    gh: Gh,
}

fn rig(name: &str) -> Rig {
    let home = Scratch::new(&format!("{name}-home"));
    let gh = Gh::new(&format!("{name}-gh"));
    gh.respond("repos/acme/widgets", &json!({ "default_branch": "main" }));
    gh.respond(
        "repos/acme/widgets/commits/main",
        &json!({ "sha": "0123456789abcdef0123456789abcdef01234567" }),
    );
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
    let rig = Rig { home, gh };
    rig.write_executor(0, "", &events(), &record("OUTCOME_COMPLETED", ""));
    rig
}

fn events() -> String {
    [
        json!({"workloadId": "w", "sequence": "1", "time": "2026-11-18T07:00:00Z", "started": {"executorVersion": "0.0.0"}}),
        json!({"workloadId": "w", "sequence": "2", "time": "2026-11-18T07:00:01Z", "workspaceReady": {"path": "/x/work/w", "commit": "0123456789abcdef0123456789abcdef01234567"}}),
        json!({"workloadId": "w", "sequence": "3", "time": "2026-11-18T07:00:02Z", "agentMessage": {"text": "I will read the README.\nThen the workflow."}}),
        json!({"workloadId": "w", "sequence": "4", "time": "2026-11-18T07:00:03Z", "toolCall": {"id": "t1", "name": "Read", "input": {"file_path": "/x/work/w/README.md"}}}),
        json!({"workloadId": "w", "sequence": "5", "time": "2026-11-18T07:00:04Z", "toolResult": {"callId": "t1", "output": "the README's whole text, which is not shown"}}),
        json!({"workloadId": "w", "sequence": "6", "time": "2026-11-18T07:00:05Z", "heartbeat": {}}),
        json!({"workloadId": "w", "sequence": "7", "time": "2026-11-18T07:00:06Z", "gateStarted": {"name": "format"}}),
        json!({"workloadId": "w", "sequence": "8", "time": "2026-11-18T07:00:07Z", "gateFinished": {"name": "format", "result": {"gate": "format", "outcome": "skipped", "reason": "no changed file matches"}}}),
        json!({"workloadId": "w", "sequence": "9", "time": "2026-11-18T07:00:08Z", "cost": {"unit": "COST_UNIT_USD", "amount": 0.125, "source": "SOURCE_REPORTED_BY_RUNTIME"}}),
        json!({"workloadId": "w", "sequence": "10", "time": "2026-11-18T07:00:09Z", "pullRequestOpened": {"url": PR_URL, "number": "52", "headBranch": "heph/12-make-the-reset-spec"}}),
        json!({"workloadId": "w", "sequence": "11", "time": "2026-11-18T07:00:10Z", "finished": {"outcome": "OUTCOME_COMPLETED"}}),
    ]
    .iter()
    .map(ToString::to_string)
    .collect::<Vec<_>>()
    .join("\n")
        + "\n"
}

fn record(outcome: &str, reason: &str) -> String {
    json!({
        "workloadId": "w",
        "outcome": outcome,
        "reason": reason,
        "costs": [
            {"unit": "COST_UNIT_USD", "amount": 0.125, "source": "SOURCE_REPORTED_BY_RUNTIME"},
            {"unit": "COST_UNIT_TOKENS", "amount": 144_096, "source": "SOURCE_REPORTED_BY_RUNTIME"}
        ],
        "pullRequest": {"url": PR_URL, "number": "52", "headBranch": "heph/12-make-the-reset-spec"},
        "eventCount": "11"
    })
    .to_string()
}

impl Rig {
    fn executor(&self) -> PathBuf {
        self.home.0.join("executor-dir/executor")
    }

    fn seen(&self, file: &str) -> String {
        std::fs::read_to_string(self.home.0.join(format!("executor-dir/seen/{file}")))
            .unwrap_or_default()
    }

    /// A stand-in executor: it records its arguments and the secret pointers it
    /// was given, prints `events`, writes `record` where it was told, says
    /// `stderr` and exits with `code`.
    fn write_executor(&self, code: i32, stderr: &str, events: &str, record: &str) {
        let dir = self.home.0.join("executor-dir");
        std::fs::create_dir_all(dir.join("seen")).expect("a directory");
        std::fs::write(dir.join("events.canned"), events).expect("events");
        std::fs::write(dir.join("record.canned"), record).expect("a record");
        std::fs::write(dir.join("stderr.canned"), stderr).expect("stderr");
        let script = format!(
            "#!/bin/sh\n\
             d={dir}\n\
             printf '%s\\n' \"$@\" > $d/seen/args.txt\n\
             printf '%s\\n' \"${{HEPH_GITHUB_TOKEN_FILE:-none}}\" > $d/seen/github-file.txt\n\
             printf '%s\\n' \"${{CLAUDE_CODE_OAUTH_TOKEN_FILE:-none}}\" > $d/seen/claude-file.txt\n\
             printf '%s\\n' \"${{HEPH_GITHUB_TOKEN:+set}}\" > $d/seen/github-variable.txt\n\
             pwd > $d/seen/cwd.txt\n\
             prev=''\n\
             for a in \"$@\"; do [ \"$prev\" = '--record' ] && rec=\"$a\"; prev=\"$a\"; done\n\
             [ -n \"$rec\" ] && cp $d/record.canned \"$rec\"\n\
             cat $d/stderr.canned >&2\n\
             cat $d/events.canned\n\
             exit {code}\n",
            dir = dir.display()
        );
        let path = self.executor();
        std::fs::write(&path, script).expect("a script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("to chmod");
    }

    fn config_dir(&self) -> PathBuf {
        self.home.0.join("config")
    }

    fn runs_dir(&self) -> PathBuf {
        self.home.0.join("runs")
    }

    fn token_file(&self, name: &str) -> PathBuf {
        let path = self
            .home
            .write(&format!("config/hephaestium/{name}"), "a-token\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("to chmod");
        path
    }

    /// `heph run`, with the home and the configuration directory this rig's, and
    /// no token anywhere unless the test gives one.
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let path = format!(
            "{}:{}",
            self.gh.bin().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let home = self.home.0.to_string_lossy().into_owned();
        let config = self.config_dir().to_string_lossy().into_owned();
        let gh_dir = self.gh.scratch.0.to_string_lossy().into_owned();
        let mut all: Vec<(&str, &str)> = vec![
            ("PATH", &path),
            ("HOME", &home),
            ("XDG_CONFIG_HOME", &config),
            ("XDG_STATE_HOME", ""),
            ("HEPH_TEST_GH_DIR", &gh_dir),
            ("HEPH_GITHUB_TOKEN", ""),
            ("HEPH_GITHUB_TOKEN_FILE", ""),
            ("CLAUDE_CODE_OAUTH_TOKEN", ""),
            ("CLAUDE_CODE_OAUTH_TOKEN_FILE", ""),
        ];
        all.extend_from_slice(env);
        let executor = self.executor().to_string_lossy().into_owned();
        let runs = self.runs_dir().to_string_lossy().into_owned();
        let mut arguments = vec!["run"];
        arguments.extend_from_slice(args);
        if !args.contains(&"--executor") {
            arguments.extend(["--executor", &executor]);
        }
        if !args.contains(&"--runs-dir") {
            arguments.extend(["--runs-dir", &runs]);
        }
        heph_with_env(&self.home.0, &arguments, &all)
    }

    /// The one run directory under the runs directory, if exactly one was made.
    fn run_directory(&self) -> Option<PathBuf> {
        let repository = std::fs::read_dir(self.runs_dir().join("acme-widgets")).ok()?;
        let mut found: Vec<PathBuf> = repository.flatten().map(|e| e.path()).collect();
        (found.len() == 1).then(|| found.remove(0))
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn a_run_keeps_its_own_directory_starts_the_executor_shows_progress_and_ends_with_the_pull_request()
{
    let rig = rig("run-ok");
    rig.token_file("github-token");

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let directory = rig
        .run_directory()
        .expect("one run directory under acme-widgets");
    let name = directory
        .file_name()
        .expect("a name")
        .to_string_lossy()
        .into_owned();
    assert!(name.starts_with("12-"), "{name}");
    assert!(directory.starts_with(rig.runs_dir()));

    // What the run kept.
    let workload: serde_json::Value =
        serde_json::from_str(&read(&directory.join("workload.json"))).expect("JSON");
    assert_eq!(workload["ticket"]["number"], "12");
    assert_eq!(
        read(&directory.join("events.jsonl")),
        events(),
        "the events are kept exactly as they came"
    );
    assert_eq!(
        read(&directory.join("record.json")),
        record("OUTCOME_COMPLETED", "")
    );
    assert!(directory.join("executor.log").exists());

    // How the executor was started.
    let args = rig.seen("args.txt");
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(args[0], "run");
    let flag = |name: &str| -> String {
        let at = args
            .iter()
            .position(|a| *a == name)
            .unwrap_or_else(|| panic!("no {name}: {args:?}"));
        args[at + 1].to_owned()
    };
    assert_eq!(
        flag("--workload"),
        directory.join("workload.json").display().to_string()
    );
    assert_eq!(
        flag("--work-dir"),
        directory.join("work").display().to_string()
    );
    assert_eq!(
        flag("--record"),
        directory.join("record.json").display().to_string()
    );
    assert!(args.contains(&"--deliver"), "{args:?}");

    // What it showed.
    let shown = stdout(&output);
    for expected in [
        "workspace ready",
        "agent",
        "I will read the README.",
        "tool",
        "Read",
        "README.md",
        "gate format",
        "skipped",
        PR_URL,
    ] {
        assert!(shown.contains(expected), "no '{expected}' in:\n{shown}");
    }
    assert!(shown.contains("completed"), "{shown}");
    assert!(shown.contains("$0.125"), "the cost: {shown}");
    assert!(
        shown.contains("144096") || shown.contains("144,096"),
        "the tokens: {shown}"
    );
    assert!(
        shown.contains(&directory.display().to_string()),
        "where the run is kept: {shown}"
    );
}

#[test]
fn progress_shows_what_matters_and_not_the_rest() {
    let rig = rig("run-progress");
    rig.token_file("github-token");

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    let shown = stdout(&output);
    assert!(
        shown.contains("workspace ready") && shown.contains("Read"),
        "nothing was shown at all:\n{shown}"
    );
    assert!(
        !shown.contains("whole text"),
        "a tool result was shown:\n{shown}"
    );
    assert!(
        !shown.contains("heartbeat"),
        "a heartbeat was shown:\n{shown}"
    );
    assert!(
        !shown.contains("Then the workflow."),
        "more than the first line of a message:\n{shown}"
    );
    assert!(
        !shown.contains("\"sequence\""),
        "raw events were shown:\n{shown}"
    );
}

#[test]
fn the_runs_directory_is_the_flag_else_the_xdg_state_directory_else_the_home_directorys() {
    // The flag is what every other test uses. Without it:
    let rig_xdg = rig("run-xdg");
    rig_xdg.token_file("github-token");
    let state = rig_xdg.home.0.join("state");
    let output = rig_xdg.run(
        &["12", "--repo", "acme/widgets", "--runs-dir", ""],
        &[("XDG_STATE_HOME", &state.to_string_lossy())],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        state.join("hephaestium/runs/acme-widgets").is_dir(),
        "{}",
        stdout(&output)
    );

    let rig_home = rig("run-home");
    rig_home.token_file("github-token");
    let output = rig_home.run(&["12", "--repo", "acme/widgets", "--runs-dir", ""], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        rig_home
            .home
            .0
            .join(".local/state/hephaestium/runs/acme-widgets")
            .is_dir(),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_default_token_file_is_offered_to_the_executor_and_what_is_already_set_is_not_overridden() {
    // Both defaults exist and nothing is set: both are offered.
    let rig = rig("run-tokens");
    let github = rig.token_file("github-token");
    let claude = rig.token_file("claude-token");
    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        rig.seen("github-file.txt").trim(),
        github.display().to_string()
    );
    assert_eq!(
        rig.seen("claude-file.txt").trim(),
        claude.display().to_string()
    );

    // A _FILE variable already set is kept.
    let elsewhere = rig.home.write("elsewhere", "x");
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o600)).expect("to chmod");
    let output = rig.run(
        &["12", "--repo", "acme/widgets"],
        &[("HEPH_GITHUB_TOKEN_FILE", &elsewhere.to_string_lossy())],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        rig.seen("github-file.txt").trim(),
        elsewhere.display().to_string()
    );

    // A variable already set means no file is offered for it.
    let output = rig.run(
        &["12", "--repo", "acme/widgets"],
        &[("HEPH_GITHUB_TOKEN", "a-token-value")],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        rig.seen("github-file.txt").trim(),
        "none",
        "a file was offered beside a variable"
    );
    assert_eq!(
        rig.seen("github-variable.txt").trim(),
        "set",
        "the variable was not passed on"
    );
}

#[test]
fn a_missing_github_token_ends_the_run_before_anything_is_made_and_says_where_to_put_one() {
    let rig = rig("run-no-token");

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("HEPH_GITHUB_TOKEN"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("hephaestium/github-token"),
        "{}",
        stderr(&output)
    );
    assert!(!rig.runs_dir().exists(), "a directory was made");
    assert_eq!(rig.seen("args.txt"), "", "the executor was started");
}

#[test]
fn a_ticket_that_is_not_open_ends_the_run_before_anything_is_made() {
    let rig = rig("run-closed");
    rig.token_file("github-token");
    rig.gh.respond(
        "repos/acme/widgets/issues/12",
        &json!({"number": 12, "title": "t", "body": "b", "html_url": "u", "state": "closed"}),
    );

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(stderr(&output).contains("not open"), "{}", stderr(&output));
    assert!(!rig.runs_dir().exists());
    assert_eq!(rig.seen("args.txt"), "");
}

#[test]
fn a_workload_that_did_not_complete_exits_1_and_says_why() {
    let rig = rig("run-failed");
    rig.token_file("github-token");
    let reason = "gate test failed";
    let failed = events().replace(
        r#""finished":{"outcome":"OUTCOME_COMPLETED"}"#,
        &format!(r#""finished":{{"outcome":"OUTCOME_FAILED","reason":"{reason}"}}"#),
    );
    rig.write_executor(0, "", &failed, &record("OUTCOME_FAILED", reason));

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).contains("failed"), "{}", stdout(&output));
    assert!(stdout(&output).contains(reason), "{}", stdout(&output));
}

#[test]
fn an_executor_that_cannot_be_found_or_refuses_the_workload_exits_3_and_says_what_it_said() {
    let rig = rig("run-executor");
    rig.token_file("github-token");

    let missing = rig.run(
        &[
            "12",
            "--repo",
            "acme/widgets",
            "--executor",
            "/nowhere/executor",
        ],
        &[],
    );
    assert_eq!(missing.status.code(), Some(3), "{}", stderr(&missing));
    assert!(
        stderr(&missing).contains("executor"),
        "{}",
        stderr(&missing)
    );

    rig.write_executor(
        1,
        "executor: the base commit is not forty hexadecimal digits\n",
        "",
        "",
    );
    let refused = rig.run(&["12", "--repo", "acme/widgets"], &[]);
    assert_eq!(refused.status.code(), Some(3), "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("the base commit is not forty hexadecimal digits"),
        "what the executor said must be shown: {}",
        stderr(&refused)
    );
}

#[test]
fn a_usage_error_is_exit_2() {
    let rig = rig("run-usage");
    for args in [
        vec![],
        vec!["twelve"],
        vec!["12", "--budget-usd", "0"],
        vec!["12", "--nonsense"],
    ] {
        let output = rig.run(&args, &[]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("run TICKET"),
            "the usage error must be about run: {}",
            stderr(&output)
        );
    }
}

#[test]
fn the_summary_names_the_pull_request_from_the_record_and_not_only_from_the_events() {
    let rig = rig("run-summary-pr");
    rig.token_file("github-token");
    let without_the_event = events()
        .lines()
        .filter(|line| !line.contains("pullRequestOpened"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    rig.write_executor(0, "", &without_the_event, &record("OUTCOME_COMPLETED", ""));

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains(&format!("pull request: {PR_URL}")),
        "{}",
        stdout(&output)
    );
}
