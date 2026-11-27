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
             prev=''\n\
             for a in \"$@\"; do [ \"$prev\" = '--work-dir' ] && work=\"$a\"; prev=\"$a\"; done\n\
             [ -n \"$work\" ] && mkdir -p \"$work/repo\" && echo checkout > \"$work/repo/file\"\n\
             [ -n \"$work\" ] && for b in target _build deps node_modules; do mkdir -p \"$work/repo/$b\" && echo built > \"$work/repo/$b/object\"; done\n\
             [ -n \"$work\" ] && mkdir -p \"$work/.tool-home\" && echo cache > \"$work/.tool-home/cache\"\n\
             [ -f $d/undeletable ] && chmod 500 \"$work/repo\"\n\
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
        vec!["12", "--approver", "someone"],
        vec!["12", "--approved-at", "2026-11-17T12:00:00Z"],
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

impl Rig {
    /// A stand-in executor that says it has started, and when it is told to stop
    /// (terminate or interrupt) records that it was, writes a cancelled
    /// workload's events and record, and exits as the real one does.
    fn write_executor_that_waits_to_be_stopped(&self) {
        let dir = self.home.0.join("executor-dir");
        std::fs::create_dir_all(dir.join("seen")).expect("a directory");
        let cancelled_events = events().replace(
            r#""finished":{"outcome":"OUTCOME_COMPLETED"}"#,
            r#""finished":{"outcome":"OUTCOME_CANCELLED","reason":"cancelled by a signal"}"#,
        );
        std::fs::write(dir.join("events.canned"), cancelled_events).expect("events");
        std::fs::write(
            dir.join("record.canned"),
            record("OUTCOME_CANCELLED", "cancelled by a signal"),
        )
        .expect("a record");
        let script = format!(
            "#!/bin/sh\n\
             d={dir}\n\
             prev=''\n\
             for a in \"$@\"; do [ \"$prev\" = '--record' ] && rec=\"$a\"; prev=\"$a\"; done\n\
             stop() {{ touch $d/seen/stopped; cp $d/record.canned \"$rec\"; cat $d/events.canned; exit 0; }}\n\
             trap stop TERM INT\n\
             touch $d/seen/ready\n\
             while :; do sleep 0.2; done\n",
            dir = dir.display()
        );
        let path = self.executor();
        std::fs::write(&path, script).expect("a script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("to chmod");
    }

    /// Start `heph run` and leave it running.
    fn start(&self, args: &[&str]) -> std::process::Child {
        let path = format!(
            "{}:{}",
            self.gh.bin().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let executor = self.executor().to_string_lossy().into_owned();
        let runs = self.runs_dir().to_string_lossy().into_owned();
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_heph"));
        common::without_heph_variables(&mut command);
        command
            .current_dir(&self.home.0)
            .arg("run")
            .args(args)
            .args(["--executor", &executor, "--runs-dir", &runs])
            .env("PATH", path)
            .env("HOME", &self.home.0)
            .env("XDG_CONFIG_HOME", self.config_dir())
            .env("XDG_STATE_HOME", "")
            .env("HEPH_TEST_GH_DIR", &self.gh.scratch.0)
            .env("HEPH_GITHUB_TOKEN", "")
            .env("HEPH_GITHUB_TOKEN_FILE", "")
            .env("CLAUDE_CODE_OAUTH_TOKEN", "")
            .env("CLAUDE_CODE_OAUTH_TOKEN_FILE", "")
            .stdout(std::fs::File::create(self.home.0.join("heph-out.txt")).expect("a file"))
            .stderr(std::fs::File::create(self.home.0.join("heph-err.txt")).expect("a file"));
        command.spawn().expect("heph to start")
    }
}

#[test]
fn a_signal_to_heph_run_is_passed_to_the_executor_and_the_run_ends_cancelled() {
    for signal in ["TERM", "INT"] {
        let rig = rig(&format!("run-cancel-{signal}"));
        rig.token_file("github-token");
        rig.write_executor_that_waits_to_be_stopped();
        let mut heph = rig.start(&["12", "--repo", "acme/widgets"]);
        let ready = rig.home.0.join("executor-dir/seen/ready");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !ready.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the executor never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        std::process::Command::new("kill")
            .args([format!("-{signal}"), heph.id().to_string()])
            .status()
            .expect("kill");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let status = loop {
            if let Some(status) = heph.try_wait().expect("to wait") {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{signal}: heph did not end within 30 seconds"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let shown = std::fs::read_to_string(rig.home.0.join("heph-out.txt")).unwrap_or_default();
        let said = std::fs::read_to_string(rig.home.0.join("heph-err.txt")).unwrap_or_default();

        assert_eq!(status.code(), Some(1), "{signal}: {said}\n{shown}");
        assert!(
            rig.home.0.join("executor-dir/seen/stopped").exists(),
            "{signal}: the executor was not told to stop"
        );
        assert!(shown.contains("outcome: cancelled"), "{signal}: {shown}");
    }
}

#[test]
fn a_run_holds_its_directory_while_it_runs_and_lets_go_when_it_ends() {
    let rig = rig("run-holds");
    rig.token_file("github-token");
    rig.write_executor_that_waits_to_be_stopped();
    let mut heph = rig.start(&["12", "--repo", "acme/widgets"]);
    let ready = rig.home.0.join("executor-dir/seen/ready");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ready.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the executor never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let directory = rig.run_directory().expect("one run directory");
    let held_while_running = heph::run_lock::is_held(&directory);

    std::process::Command::new("kill")
        .args(["-TERM", &heph.id().to_string()])
        .status()
        .expect("kill");
    heph.wait().expect("heph to end");
    let held_after = heph::run_lock::is_held(&directory);

    assert!(
        held_while_running,
        "a sweep by a worker would take this live run for a finished one"
    );
    assert!(!held_after, "the run's directory stays held after it ended");
}

// What a run leaves behind, and what deleting it may touch.

const OLD_ID: &str = "11111111-2222-4333-8444-555555555555";

impl Rig {
    /// A run of an earlier `heph run`: its directory under the runs directory, its record
    /// (if it has one) saying how it ended, and a checkout, which ended `hours` ago.
    fn earlier_run(&self, name: &str, record: Option<&str>, hours: u64) -> PathBuf {
        let directory = self.runs_dir().join("acme-widgets").join(name);
        std::fs::create_dir_all(directory.join("work/repo")).expect("a checkout");
        std::fs::write(directory.join("work/repo/file"), "checkout").expect("a file");
        for kept in ["workload.json", "events.jsonl", "executor.log"] {
            std::fs::write(directory.join(kept), "kept").expect("a kept file");
        }
        if let Some(text) = record {
            let path = directory.join("record.json");
            std::fs::write(&path, text).expect("a record");
            let ended = std::time::SystemTime::now() - std::time::Duration::from_secs(hours * 3600);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .expect("to open the record")
                .set_modified(ended)
                .expect("to date the record");
        }
        directory
    }

    fn started_runs(&self) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(self.runs_dir().join("acme-widgets"))
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        found.sort();
        found
    }
}

fn run_named(number: u32) -> String {
    format!("{number}-{OLD_ID}")
}

#[test]
fn a_run_that_completed_deletes_its_checkout_and_keeps_what_it_said() {
    let rig = rig("cleanup-completed");
    rig.token_file("github-token");

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let directory = rig.run_directory().expect("one run directory");
    assert!(!directory.join("work").exists(), "the checkout is deleted");
    assert!(
        rig.seen("args.txt").contains("--work-dir"),
        "the executor was given one to make"
    );
    for kept in [
        "workload.json",
        "events.jsonl",
        "executor.log",
        "record.json",
    ] {
        assert!(directory.join(kept).exists(), "{kept} is kept");
    }
}

#[test]
fn a_run_that_did_not_complete_keeps_its_checkout() {
    let rig = rig("cleanup-failed");
    rig.token_file("github-token");
    rig.write_executor(0, "", &events(), &record("OUTCOME_FAILED", "a gate failed"));

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let directory = rig.run_directory().expect("one run directory");
    assert!(directory.join("work/repo/file").exists());
}

#[test]
fn the_next_run_deletes_the_checkouts_that_are_due_and_no_others() {
    let rig = rig("cleanup-sweep");
    rig.token_file("github-token");
    let failed = record("OUTCOME_FAILED", "a gate failed");
    let completed = record("OUTCOME_COMPLETED", "");
    let old_failed = rig.earlier_run(&run_named(1), Some(&failed), 73);
    let recent_failed = rig.earlier_run(&run_named(2), Some(&failed), 71);
    let leftover_completed = rig.earlier_run(&run_named(3), Some(&completed), 0);

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!old_failed.join("work").exists(), "past the window");
    assert!(recent_failed.join("work/repo/file").exists(), "inside it");
    assert!(
        !leftover_completed.join("work").exists(),
        "a completed run has no use for its checkout"
    );
    for directory in [&old_failed, &recent_failed, &leftover_completed] {
        assert!(directory.join("events.jsonl").exists());
        assert!(directory.join("record.json").exists() || directory == &old_failed);
    }
}

#[test]
fn a_run_that_may_still_be_going_or_is_not_a_run_is_not_touched() {
    let rig = rig("cleanup-untouched");
    rig.token_file("github-token");
    let failed = record("OUTCOME_FAILED", "");
    let no_record = rig.earlier_run(&run_named(1), None, 0);
    let not_json = rig.earlier_run(&run_named(2), Some("not json"), 500);
    let unspecified = rig.earlier_run(
        &run_named(3),
        Some("{\"outcome\": \"OUTCOME_UNSPECIFIED\"}"),
        500,
    );
    let no_outcome = rig.earlier_run(&run_named(4), Some("{}"), 500);
    let badly_named = rig.earlier_run("notes", Some(&failed), 500);
    let not_a_uuid = rig.earlier_run("5-not-a-workload-id", Some(&failed), 500);

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    for directory in [
        no_record,
        not_json,
        unspecified,
        no_outcome,
        badly_named,
        not_a_uuid,
    ] {
        assert!(
            directory.join("work/repo/file").exists(),
            "{} was touched",
            directory.display()
        );
    }
}

#[test]
fn a_symlink_is_removed_and_never_followed() {
    let rig = rig("cleanup-symlink");
    rig.token_file("github-token");
    let outside = rig.home.0.join("outside");
    std::fs::create_dir_all(&outside).expect("a directory");
    std::fs::write(outside.join("precious"), "keep me").expect("a file");
    let completed = record("OUTCOME_COMPLETED", "");

    // A checkout that is a link out of the runs directory.
    let linked = rig.earlier_run(&run_named(1), Some(&completed), 0);
    std::fs::remove_dir_all(linked.join("work")).expect("to remove");
    std::os::unix::fs::symlink(&outside, linked.join("work")).expect("a link");

    // A run directory that is itself a link out.
    let outside_run = rig.home.0.join("outside-run");
    std::fs::create_dir_all(outside_run.join("work")).expect("a directory");
    std::fs::write(outside_run.join("record.json"), &completed).expect("a record");
    std::os::unix::fs::symlink(
        &outside_run,
        rig.runs_dir().join("acme-widgets").join(run_named(2)),
    )
    .expect("a link");

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(read(&outside.join("precious")), "keep me");
    assert!(!linked.join("work").exists(), "the link itself is gone");
    assert!(
        outside_run.join("work").exists(),
        "a linked run is not entered"
    );
}

#[test]
fn the_window_is_the_flag_else_the_environment_else_72_hours() {
    let rig = rig("cleanup-window");
    rig.token_file("github-token");
    let failed = record("OUTCOME_FAILED", "");
    let two_hours = rig.earlier_run(&run_named(1), Some(&failed), 2);

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(two_hours.join("work").exists(), "72 hours by default");

    let output = rig.run(
        &["12", "--repo", "acme/widgets"],
        &[("HEPH_WORKSPACE_KEEP_HOURS", "1")],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!two_hours.join("work").exists(), "the environment's window");

    let kept = rig.earlier_run(&run_named(2), Some(&failed), 2);
    let output = rig.run(
        &["12", "--repo", "acme/widgets", "--keep-hours", "5"],
        &[("HEPH_WORKSPACE_KEEP_HOURS", "1")],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        kept.join("work").exists(),
        "the flag wins over the environment"
    );
}

#[test]
fn a_window_that_is_not_a_whole_number_of_hours_ends_the_run_before_anything_is_made() {
    let rig = rig("cleanup-bad-window");
    rig.token_file("github-token");

    for bad in ["soon", "-1", "1.5"] {
        let output = rig.run(
            &["12", "--repo", "acme/widgets"],
            &[("HEPH_WORKSPACE_KEEP_HOURS", bad)],
        );
        assert_eq!(output.status.code(), Some(3), "{bad}: {}", stderr(&output));
        assert!(
            stderr(&output).contains("HEPH_WORKSPACE_KEEP_HOURS"),
            "{bad}"
        );
        assert!(rig.started_runs().is_empty(), "{bad}: nothing was made");
    }
}

#[test]
fn a_checkout_that_cannot_be_deleted_is_a_warning_and_does_not_change_how_the_run_ended() {
    let rig = rig("cleanup-undeletable");
    rig.token_file("github-token");
    let completed = record("OUTCOME_COMPLETED", "");
    let stuck = rig.earlier_run(&run_named(1), Some(&completed), 0);
    let inner = stuck.join("work/repo");
    std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o500)).expect("to chmod");

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o700)).expect("to restore");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("could not delete the checkout"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn progress_says_which_agent_is_running_and_what_a_reader_found() {
    let started = |kind: &str, round: u32, lens: &str| {
        let mut step = json!({ "kind": kind, "round": round });
        if !lens.is_empty() {
            step["lens"] = json!(lens);
        }
        json!({ "workloadId": "w", "sequence": "3", "time": "2026-11-22T07:00:00Z", "stepStarted": step })
    };
    let finished = |lens: &str, verdict: &str| {
        let mut step = json!({ "kind": "KIND_REVIEW", "round": 1, "lens": lens });
        if !verdict.is_empty() {
            step["verdict"] = json!(verdict);
        }
        json!({ "workloadId": "w", "sequence": "9", "time": "2026-11-22T07:00:01Z", "stepFinished": step })
    };

    assert_eq!(
        heph::run::render(&started("KIND_IMPLEMENTATION", 0, "")).as_deref(),
        Some("implementing")
    );
    assert_eq!(
        heph::run::render(&started("KIND_REVIEW", 1, "rust")).as_deref(),
        Some("reading under lens rust, round 1")
    );
    assert_eq!(
        heph::run::render(&started("KIND_REVISION", 2, "")).as_deref(),
        Some("fixing what blocked, round 2")
    );
    assert_eq!(
        heph::run::render(&finished("rust", "needs_revision")).as_deref(),
        Some("lens rust: needs_revision")
    );
    assert_eq!(
        heph::run::render(&finished("spec", "")).as_deref(),
        Some("lens spec: no verdict")
    );
    // The end of the implementation or of a fix is not worth a line: what follows says what is next.
    let end_of_implementation = json!({
        "workloadId": "w", "sequence": "4", "time": "2026-11-22T07:00:02Z",
        "stepFinished": { "kind": "KIND_IMPLEMENTATION" }
    });
    assert_eq!(heph::run::render(&end_of_implementation), None);
}

/// The canned record with `steps` added.
fn record_with_steps(steps: &serde_json::Value) -> String {
    let mut value: serde_json::Value =
        serde_json::from_str(&record("OUTCOME_COMPLETED", "")).expect("the canned record is JSON");
    value["steps"] = steps.clone();
    value.to_string()
}

fn usd(amount: f64) -> serde_json::Value {
    json!({"unit": "COST_UNIT_USD", "amount": amount, "source": "SOURCE_REPORTED_BY_RUNTIME"})
}

fn tokens(amount: u64) -> serde_json::Value {
    json!({"unit": "COST_UNIT_TOKENS", "amount": amount, "source": "SOURCE_REPORTED_BY_RUNTIME"})
}

/// The lines of the summary after `cost:` and before `pull request:`.
fn step_lines(shown: &str) -> Vec<String> {
    shown
        .lines()
        .skip_while(|l| !l.starts_with("cost:"))
        .skip(1)
        .take_while(|l| !l.starts_with("pull request:"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_summary_lists_each_step_in_order_with_what_it_ended_in_and_what_it_spent() {
    let rig = rig("run-steps");
    rig.token_file("github-token");
    let steps = json!([
        {"kind": "KIND_IMPLEMENTATION", "costs": [usd(0.5)]},
        {"kind": "KIND_REVIEW", "lens": "rust", "round": 1, "verdict": "needs_revision", "costs": [usd(0.2)]},
        {"kind": "KIND_REVIEW", "lens": "docs", "round": 1, "costs": [usd(0.1)]},
        {"kind": "KIND_REVISION", "round": 1, "costs": [usd(0.3)]}
    ]);
    rig.write_executor(0, "", &events(), &record_with_steps(&steps));

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        step_lines(&stdout(&output)),
        [
            "  implementation: $0.500",
            "  review rust round 1: needs_revision, $0.200",
            "  review docs round 1: no verdict, $0.100",
            "  revision round 1: $0.300",
        ],
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_step_with_dollars_and_tokens_shows_both_and_never_adds_them() {
    let rig = rig("run-steps-units");
    rig.token_file("github-token");
    let steps = json!([
        {"kind": "KIND_REVIEW", "lens": "rust", "round": 2, "verdict": "approved", "costs": [usd(0.25), tokens(1200)]}
    ]);
    rig.write_executor(0, "", &events(), &record_with_steps(&steps));

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    assert_eq!(
        step_lines(&stdout(&output)),
        ["  review rust round 2: approved, $0.250, 1200 tokens"],
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_record_with_no_steps_prints_no_step_lines() {
    let rig = rig("run-no-steps");
    rig.token_file("github-token");

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    let shown = stdout(&output);
    assert!(
        step_lines(&shown).is_empty(),
        "step lines without steps:\n{shown}"
    );
    assert!(
        shown.contains("cost: $0.125, 144096 tokens\npull request: "),
        "the summary changed:\n{shown}"
    );
}

#[test]
fn a_run_that_failed_timed_out_or_was_blocked_loses_its_build_output_and_keeps_the_rest() {
    for outcome in ["OUTCOME_FAILED", "OUTCOME_TIMED_OUT", "OUTCOME_BLOCKED"] {
        let rig = rig(&format!("build-output-{}", outcome.to_lowercase()));
        rig.token_file("github-token");
        rig.write_executor(0, "", &events(), &record(outcome, "it did not complete"));

        let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

        assert_eq!(
            output.status.code(),
            Some(1),
            "{outcome}: {}",
            stderr(&output)
        );
        let directory = rig.run_directory().expect("one run directory");
        assert!(
            directory.join("work/repo/file").exists(),
            "{outcome}: source"
        );
        for built in ["target", "_build", "deps", "node_modules"] {
            assert!(
                !directory.join("work/repo").join(built).exists(),
                "{outcome}: {built}"
            );
        }
        assert!(
            !directory.join("work/.tool-home").exists(),
            "{outcome}: .tool-home"
        );
        for kept in [
            "workload.json",
            "events.jsonl",
            "executor.log",
            "record.json",
        ] {
            assert!(directory.join(kept).exists(), "{outcome}: {kept}");
        }
    }
}

#[test]
fn a_run_that_completed_is_not_asked_to_discard_its_build_output() {
    // The checkout is made undeletable, so what a completed run does is left to be seen:
    // it tries to delete the checkout and says so, and never goes to discard the build
    // output, which would be a second complaint and would take what it could.
    let rig = rig("build-output-completed");
    rig.token_file("github-token");
    std::fs::write(rig.home.0.join("executor-dir/undeletable"), "").expect("a marker");

    let output = rig.run(&["12", "--repo", "acme/widgets"], &[]);

    let directory = rig.run_directory().expect("one run directory");
    let inner = directory.join("work/repo");
    std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o700)).expect("to restore");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let shown = stderr(&output);
    assert!(shown.contains("could not delete the checkout"), "{shown}");
    assert!(!shown.contains("could not remove"), "{shown}");
    assert!(inner.join("target/object").is_file(), "{shown}");
}
