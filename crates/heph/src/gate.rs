//! Running a gate and reporting what happened.
//!
//! A gate is a command the manifest declares. Running one produces a gate
//! result, whose shape is the contract in Athenaeum's `gate-result` schema,
//! pinned at the tag this crate builds against.

use std::io::Read as _;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobSetBuilder};
use rustix::process::{Pid, Signal, kill_process_group};
use serde::Serialize;
use serde_json::Value;

use crate::manifest::{Gate, Manifest, Requirement, ResultKind};

/// The exit code a shell reports for a program that does not exist.
const NOT_FOUND: i32 = 127;

/// The exit code a shell reports for a file that cannot be executed.
const NOT_EXECUTABLE: i32 = 126;

/// What a shell reports for a command killed by a signal, plus the signal.
const KILLED_BY_SIGNAL: i32 = 128;

/// How often a running gate is asked whether it has finished.
const POLL: Duration = Duration::from_millis(10);

/// How a gate ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The command exited 0.
    Passed,
    /// The command exited non-zero.
    Failed,
    /// The command was stopped for running longer than the gate allows.
    TimedOut,
    /// The command was not run.
    Skipped,
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::TimedOut => "timed out",
            Self::Skipped => "skipped",
        })
    }
}

/// What a gate reports. The fields, and the rules between them, are those of
/// Athenaeum's gate result schema; the tests hold every result to it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GateResult {
    schema_version: u8,
    /// The name the gate has in the manifest.
    pub gate: String,
    /// How it ended.
    pub outcome: Outcome,
    /// The program and arguments, with the toolchain wrapper applied.
    pub command: Vec<String>,
    /// The command's exit code, for `passed` and `failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// How long it took, in milliseconds.
    pub duration_ms: u64,
    /// Why it was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The gate's own structured result, when it gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<Value>,
}

impl GateResult {
    /// The exit code `heph` itself ends with: 0 when the gate passed or was
    /// skipped, 1 when it failed or timed out.
    #[must_use]
    pub fn exit_code_of_heph(&self) -> u8 {
        match self.outcome {
            Outcome::Passed | Outcome::Skipped => 0,
            Outcome::Failed | Outcome::TimedOut => 1,
        }
    }
}

/// Why a gate could not be run at all, as distinct from running and failing.
#[derive(Debug)]
pub enum CouldNotRun {
    /// A pattern in `when_paths` is not a glob.
    BadGlob(String),
    /// Waiting for the command failed.
    Wait(std::io::Error),
    /// A gate that passed did not give the report the manifest says it gives.
    UnusableReport(String),
}

impl std::fmt::Display for CouldNotRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadGlob(message) => {
                write!(formatter, "when_paths holds a bad pattern: {message}")
            }
            Self::Wait(error) => write!(formatter, "waiting for the command failed: {error}"),
            Self::UnusableReport(message) => formatter.write_str(message),
        }
    }
}

/// The command to run: the toolchain wrapper in front of the gate's own
/// command, unless the caller is already inside the toolchain.
///
/// `lookup` reads an environment variable, so a caller can say what the
/// environment is without this function touching the real one.
#[must_use]
pub fn command_for(
    manifest: &Manifest,
    gate: &Gate,
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let wrapper = manifest.toolchain.as_ref().filter(|toolchain| {
        let inside = toolchain
            .inside_env
            .as_deref()
            .and_then(&lookup)
            .is_some_and(|value| !value.is_empty());
        !inside
    });

    wrapper
        .map(|toolchain| toolchain.wrapper.iter())
        .into_iter()
        .flatten()
        .chain(gate.run.iter())
        .cloned()
        .collect()
}

/// The variables a gate declares that are not set, or are set and empty, as
/// `(name, kind)` in name order.
///
/// Only names are returned and nothing here reads a value into the result, so
/// a caller that reports them cannot print a secret.
///
/// `lookup` reads an environment variable, as for [`command_for`].
#[must_use]
pub fn missing_requirements(
    gate: &Gate,
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<(String, Requirement)> {
    gate.requires
        .iter()
        .filter(|(name, _)| lookup(name).is_none_or(|value| value.is_empty()))
        .map(|(name, kind)| (name.clone(), *kind))
        .collect()
}

/// Say what is missing, in words for a person: each name, and whether it is a
/// secret. A value is never part of it.
#[must_use]
pub fn describe_missing(missing: &[(String, Requirement)]) -> String {
    missing
        .iter()
        .map(|(name, kind)| match kind {
            Requirement::Env => format!("{name} (environment variable)"),
            Requirement::Secret => format!("{name} (secret)"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a gate applies to a change.
///
/// Without a list of changed files, every gate applies: not knowing what
/// changed is no reason to skip a check. With one, a gate that names
/// `when_paths` applies only if some changed file matches one of them.
///
/// # Errors
///
/// Returns an error when a pattern in `when_paths` is not a valid glob.
pub fn applies(gate: &Gate, changed: &[String]) -> Result<bool, CouldNotRun> {
    match gate.when_paths.as_ref().filter(|_| !changed.is_empty()) {
        Some(patterns) => matches_any(patterns, changed),
        None => Ok(true),
    }
}

/// Whether any of the changed files matches any of the patterns. A `*` does
/// not cross a directory and a `**` does, and a leading `./` is ignored.
///
/// # Errors
///
/// Returns an error when a pattern is not a valid glob.
pub fn matches_any(patterns: &[String], changed: &[String]) -> Result<bool, CouldNotRun> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|error| CouldNotRun::BadGlob(format!("{pattern}: {error}")))?;
        builder.add(glob);
    }
    let set = builder
        .build()
        .map_err(|error| CouldNotRun::BadGlob(error.to_string()))?;

    Ok(changed
        .iter()
        .any(|path| set.is_match(path.strip_prefix("./").unwrap_or(path))))
}

/// The result for a gate that was not run because the change does not touch
/// anything it applies to.
#[must_use]
pub fn skipped(name: &str, gate: &Gate, command: Vec<String>) -> GateResult {
    GateResult {
        schema_version: 0,
        gate: name.to_owned(),
        outcome: Outcome::Skipped,
        command,
        exit_code: None,
        duration_ms: 0,
        reason: Some(format!(
            "no changed file matches the gate's when_paths ({})",
            gate.when_paths.as_deref().unwrap_or_default().join(", ")
        )),
        report: None,
    }
}

/// How the command ended, before it is turned into a result.
enum Ended {
    Exited(i32),
    TimedOut,
    /// It never started. The code is the one a shell would report.
    NotRun(i32),
}

/// Run a gate's command and report how it ended.
///
/// The command runs from `directory` with no input, in a process
/// group of its own. Its standard error is passed through, and so is its
/// standard output, to this process's standard error, so that standard
/// output can carry only the result. When the gate gives a JSON report, its
/// standard output is read instead, and returned as the report.
///
/// When the command ends, or is stopped for running longer than `timeout`,
/// everything it started is stopped too, so a gate leaves nothing running.
///
/// # Errors
///
/// Returns why the gate could not be run: waiting for it failed, or a
/// passing gate did not give the report the manifest says it gives.
pub fn run(
    name: &str,
    gate: &Gate,
    command: Vec<String>,
    directory: &Path,
    notify: impl Fn(&str),
) -> Result<GateResult, CouldNotRun> {
    let wants_report = gate.result == Some(ResultKind::Json);
    let started = Instant::now();
    let (ended, captured) = execute(
        &command,
        directory,
        Duration::from_secs(gate.timeout_seconds),
        wants_report,
        notify,
    )?;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let (outcome, exit_code) = match &ended {
        Ended::Exited(0) => (Outcome::Passed, Some(0)),
        Ended::Exited(code) | Ended::NotRun(code) => (Outcome::Failed, Some(*code)),
        Ended::TimedOut => (Outcome::TimedOut, None),
    };

    let report = if wants_report && matches!(outcome, Outcome::Passed | Outcome::Failed) {
        match (report_from(captured.as_deref()), outcome) {
            (Ok(report), _) => Some(report),
            (Err(message), Outcome::Passed) => {
                return Err(CouldNotRun::UnusableReport(format!(
                    "{name} passed but {message}"
                )));
            }
            (Err(_), _) => None,
        }
    } else {
        None
    };

    Ok(GateResult {
        schema_version: 0,
        gate: name.to_owned(),
        outcome,
        command,
        exit_code,
        duration_ms,
        reason: None,
        report,
    })
}

/// Read a gate's standard output as the JSON object it reports.
fn report_from(captured: Option<&[u8]>) -> Result<Value, String> {
    let bytes = captured.unwrap_or_default();
    match serde_json::from_slice::<Value>(bytes) {
        Ok(object @ Value::Object(_)) => Ok(object),
        Ok(_) => Err("its standard output is not a JSON object".to_owned()),
        Err(error) => Err(format!(
            "its standard output is not a JSON object, because it is not JSON: {error}"
        )),
    }
}

/// Start the command, wait for it for at most `timeout`, and clean up.
fn execute(
    argv: &[String],
    directory: &Path,
    timeout: Duration,
    capture_stdout: bool,
    notify: impl Fn(&str),
) -> Result<(Ended, Option<Vec<u8>>), CouldNotRun> {
    let (program, arguments) = argv
        .split_first()
        .expect("a command to have a program: the schema requires at least one element");

    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .process_group(0);
    if capture_stdout {
        command.stdout(Stdio::piped());
    } else {
        command.stdout(Stdio::from(std::io::stderr()));
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let code = if error.kind() == std::io::ErrorKind::NotFound {
                NOT_FOUND
            } else {
                NOT_EXECUTABLE
            };
            notify(&format!("{program}: {error}"));
            return Ok((Ended::NotRun(code), None));
        }
    };

    let reader = child.stdout.take().map(|mut standard_output| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = standard_output.read_to_end(&mut bytes);
            bytes
        })
    });

    let deadline = Instant::now() + timeout;
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ended::Exited(code_of(status)),
            Ok(None) => {}
            Err(error) => {
                stop_everything_started_by(&child);
                let _ = child.wait();
                return Err(CouldNotRun::Wait(error));
            }
        }
        if Instant::now() >= deadline {
            stop_everything_started_by(&child);
            let _ = child.wait();
            break Ended::TimedOut;
        }
        std::thread::sleep(POLL);
    };

    stop_everything_started_by(&child);
    let captured = reader.map(|reader| reader.join().unwrap_or_default());

    Ok((ended, captured))
}

/// The exit code of a command that has ended: its own, or the one a shell
/// reports for a command killed by a signal.
fn code_of(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| KILLED_BY_SIGNAL + signal))
        .unwrap_or(KILLED_BY_SIGNAL)
}

/// Send a kill to the command's process group, which holds the command and
/// everything it started. Failure means the group is already gone, which is
/// what was wanted.
fn stop_everything_started_by(child: &std::process::Child) {
    if let Some(group) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
        let _ = kill_process_group(group, Signal::KILL);
    }
}
