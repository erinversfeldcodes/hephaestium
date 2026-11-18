//! What the executor's tests share: a scratch directory, a local git
//! repository to clone, a workload and a transcript, and the binary run over them.

#![allow(
    dead_code,
    reason = "each test binary uses a different part of what the tests share"
)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::{Event, RunRecord};
use serde_json::{Value, json};

/// A directory of one test's own, removed when the test ends.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("executor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a scratch directory");
        Self(directory)
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    pub fn write(&self, relative: &str, text: &str) -> PathBuf {
        let file = self.path(relative);
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("a directory");
        std::fs::write(&file, text).expect("a file");
        file
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
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
    String::from_utf8(output.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned()
}

/// A manifest with the given bootstrap steps, as TOML.
pub fn manifest(steps: &[(&[&str], Option<&str>, u64)]) -> String {
    let mut text =
        String::from("schema_version = 0\n\n[components.app]\nroot = \".\"\npaths = [\"**\"]\n");
    for (run, root, timeout) in steps {
        let words: Vec<String> = run.iter().map(|w| format!("{w:?}")).collect();
        write!(text, "\n[[bootstrap]]\nrun = [{}]\n", words.join(", ")).expect("a string");
        if let Some(root) = root {
            writeln!(text, "root = {root:?}").expect("a string");
        }
        writeln!(text, "timeout_seconds = {timeout}").expect("a string");
    }
    text
}

/// A repository with one commit that holds the manifest and a `web` directory,
/// and the commit's id.
pub fn source(scratch: &Scratch, manifest_text: &str) -> (PathBuf, String) {
    let directory = scratch.path("source");
    std::fs::create_dir_all(&directory).expect("a directory");
    git(&directory, &["init", "-q", "-b", "main"]);
    scratch.write("source/.athenaeum/manifest.toml", manifest_text);
    scratch.write("source/web/.keep", "");
    scratch.write("source/README.md", "first\n");
    git(&directory, &["add", "."]);
    git(&directory, &["commit", "-q", "-m", "first"]);
    let commit = git(&directory, &["rev-parse", "HEAD"]);
    (directory, commit)
}

/// The workload the contract's own valid case holds, started from `commit`.
pub fn workload(commit: &str) -> Value {
    let case = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../cases/runtime/Workload/valid/claude-code-subscription.json"
    );
    let mut value: Value =
        serde_json::from_str(&std::fs::read_to_string(case).expect("the case")).expect("JSON");
    value["repository"]["baseCommit"] = json!(commit);
    value
}

pub const SKILL_SHA: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

pub fn transcript() -> String {
    [
        json!({"agentMessage": {"text": "I will read the spec."}}),
        json!({"toolCall": {"id": "t1", "name": "Read", "input": {"path": "README.md"}}}),
        json!({"toolResult": {"callId": "t1", "output": "first"}}),
        json!({"skillLoaded": {"name": "scope-request", "path": ".athenaeum/skills/scope-request/SKILL.md", "sha256": SKILL_SHA}}),
        json!({"cost": {"unit": "COST_UNIT_USD", "amount": 0.5, "source": "SOURCE_REPORTED_BY_RUNTIME"}}),
        json!({"cost": {"unit": "COST_UNIT_USD", "amount": 0.25, "source": "SOURCE_REPORTED_BY_RUNTIME"}}),
        json!({"cost": {"unit": "COST_UNIT_TOKENS", "amount": 1200, "source": "SOURCE_REPORTED_BY_RUNTIME"}}),
        json!({"agentMessage": {"text": "Done."}}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
        + "\n"
}

pub struct Ran {
    pub output: Output,
    pub scratch: Scratch,
}

impl Ran {
    pub fn events(&self) -> Vec<Event> {
        String::from_utf8(self.output.stdout.clone())
            .expect("UTF-8")
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("an event ({error}): {line}"))
            })
            .collect()
    }

    pub fn record(&self) -> RunRecord {
        let text = std::fs::read_to_string(self.scratch.path("record.json")).expect("a record");
        serde_json::from_str(&text).expect("a record the contract accepts")
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    pub fn code(&self) -> i32 {
        self.output.status.code().expect("an exit code")
    }
}

pub fn run_with(
    scratch: Scratch,
    workload: &Value,
    transcript: Option<&str>,
    extra: &[&str],
) -> Ran {
    scratch.write("workload.json", &workload.to_string());
    let mut command = Command::new(env!("CARGO_BIN_EXE_executor"));
    command
        .args(["run", "--workload"])
        .arg(scratch.path("workload.json"))
        .arg("--work-dir")
        .arg(scratch.path("work"))
        .arg("--record")
        .arg(scratch.path("record.json"))
        .arg("--source")
        .arg(scratch.path("source"));
    if let Some(text) = transcript {
        command
            .arg("--transcript")
            .arg(scratch.write("transcript.jsonl", text));
    }
    command.args(extra);
    let output = command.output().expect("the executor to run");
    Ran { output, scratch }
}

pub fn kinds(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event.kind.as_ref().expect("a kind") {
            Kind::Started(_) => "started",
            Kind::WorkspaceReady(_) => "workspace_ready",
            Kind::AgentMessage(_) => "agent_message",
            Kind::ToolCall(_) => "tool_call",
            Kind::ToolResult(_) => "tool_result",
            Kind::SkillLoaded(_) => "skill_loaded",
            Kind::GateStarted(_) => "gate_started",
            Kind::GateFinished(_) => "gate_finished",
            Kind::Cost(_) => "cost",
            Kind::Heartbeat(_) => "heartbeat",
            Kind::PullRequestOpened(_) => "pull_request_opened",
            Kind::Finished(_) => "finished",
        })
        .collect()
}

pub fn finished(events: &[Event]) -> (Outcome, String) {
    match events.last().and_then(|e| e.kind.as_ref()) {
        Some(Kind::Finished(done)) => (
            Outcome::try_from(done.outcome).expect("a known outcome"),
            done.reason.clone(),
        ),
        other => panic!("the stream does not end with finished: {other:?}"),
    }
}
