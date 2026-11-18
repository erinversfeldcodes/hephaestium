//! Running a gate and reporting what happened.
//!
//! A gate is a command the manifest declares. Running one produces a gate
//! result, whose shape is the contract in Athenaeum's `gate-result` schema,
//! pinned at the tag this crate builds against.

use std::path::Path;
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobSetBuilder};
use serde::Serialize;
use serde_json::Value;

use crate::manifest::{Gate, Manifest, Requirement, ResultKind};
use crate::process::{Ended, execute};

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
    wrap(manifest, &gate.run, lookup)
}

/// A command with the toolchain wrapper in front of it, unless the caller is
/// already inside the toolchain. A gate's command and a bootstrap step's are
/// both run this way, so that each runs in the toolchain the repository pins.
///
/// `lookup` reads an environment variable, as for [`command_for`].
#[must_use]
pub fn wrap(
    manifest: &Manifest,
    command: &[String],
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
        .chain(command.iter())
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

/// The result of a check that reads a repository and does not run a command,
/// such as the baseline check. It is passed or failed, and carries what it
/// found as its report.
#[must_use]
pub fn reported(
    name: &str,
    failed: bool,
    command: Vec<String>,
    duration_ms: u64,
    report: Value,
) -> GateResult {
    GateResult {
        schema_version: 0,
        gate: name.to_owned(),
        outcome: if failed {
            Outcome::Failed
        } else {
            Outcome::Passed
        },
        command,
        exit_code: Some(i32::from(failed)),
        duration_ms,
        reason: None,
        report: Some(report),
    }
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
    )
    .map_err(CouldNotRun::Wait)?;
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

/// Why a named gate was not run at all, as distinct from running and failing.
#[derive(Debug)]
pub enum NotRun {
    /// The manifest declares no gate of that name.
    NoSuchGate {
        /// What was asked for.
        name: String,
        /// The gates the manifest does declare, in name order.
        declared: Vec<String>,
    },
    /// The gate declares something it needs that is not set. Only names and
    /// kinds are held, never a value.
    Missing(Vec<(String, Requirement)>),
    /// The gate could not be run, or reported unusably.
    Could(CouldNotRun),
}

impl std::fmt::Display for NotRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchGate { name, declared } => write!(
                formatter,
                "has no gate '{name}'; it declares: {}",
                if declared.is_empty() {
                    "none".to_owned()
                } else {
                    declared.join(", ")
                }
            ),
            Self::Missing(missing) => write!(
                formatter,
                "not run, because what it needs is not set: {}",
                describe_missing(missing)
            ),
            Self::Could(error) => write!(formatter, "{error}"),
        }
    }
}

/// Run the manifest's gate called `name`, from `directory`.
///
/// A gate whose `when_paths` match none of `changed` is skipped, and says so in
/// its result. A gate that needs a variable or secret that is not set is not
/// run: that is not a failure of the gate, and the caller must be able to tell
/// them apart. Otherwise the gate runs, with the toolchain wrapper in front of
/// its command unless the caller is already inside the toolchain.
///
/// `lookup` reads an environment variable, so a caller can say what the
/// environment is without this function touching the real one.
///
/// # Errors
///
/// Returns why the gate was not run: it does not exist, something it needs is
/// not set, or running it failed.
pub fn run_named(
    manifest: &Manifest,
    name: &str,
    changed: &[String],
    directory: &Path,
    lookup: impl Fn(&str) -> Option<String>,
    notify: impl Fn(&str),
) -> Result<GateResult, NotRun> {
    let Some(gate) = manifest.gates.get(name) else {
        return Err(NotRun::NoSuchGate {
            name: name.to_owned(),
            declared: manifest.gates.keys().cloned().collect(),
        });
    };

    let command = command_for(manifest, gate, &lookup);
    if !applies(gate, changed).map_err(NotRun::Could)? {
        return Ok(skipped(name, gate, command));
    }
    let missing = missing_requirements(gate, &lookup);
    if !missing.is_empty() {
        return Err(NotRun::Missing(missing));
    }
    run(name, gate, command, directory, notify).map_err(NotRun::Could)
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
