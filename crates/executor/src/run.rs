//! Running one workload: prepare a workspace, play the agent, say how it ended.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::{
    GateFinished, GateStarted, RunRecord, Started, Workload, WorkspaceReady,
};
use heph::gate;
use heph::manifest::{self, BootstrapStep, Manifest};
use heph::process::{Ended, execute};

use crate::adapter::{Adapter, Stub};
use crate::agent::{self, End};
use crate::cli::Options;
use crate::stream::Stream;
use crate::{transcript, workload};

/// How long cloning and checking out may take.
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

/// A workload that could not be run, as distinct from one that ran and failed.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<std::io::Error> for Refused {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

/// Run the workload `options` names, writing its events to `out`.
///
/// Everything that can be checked before anything happens is checked first, so
/// a workload that is refused leaves nothing behind. From the moment the first
/// event is written, whatever goes wrong is an outcome of the workload and
/// ends in a `finished` event.
///
/// # Errors
///
/// Returns why the workload could not be run, or why its events or its record
/// could not be written.
pub fn run(options: &Options, executor_version: &str, out: &mut impl Write) -> Result<(), Refused> {
    let text = fs::read_to_string(&options.workload)
        .map_err(|e| Refused(format!("{}: {e}", options.workload.display())))?;
    let workload = workload::read(&text)
        .map_err(|e| Refused(format!("{}: {e}", options.workload.display())))?;

    let adapter = adapter_for(options)?;

    let workspace = options.work_dir.join(&workload.id);
    if workspace.exists() {
        return Err(Refused(format!(
            "{} already exists, and a workspace is never reused",
            workspace.display()
        )));
    }

    let mut stream = Stream::new(out, &workload.id);
    stream.emit(Kind::Started(Started {
        executor_version: executor_version.to_owned(),
    }))?;

    let (outcome, reason) = match prepare(options, &workload, &workspace, &mut stream) {
        Ok(manifest) => {
            match agent::run(
                adapter.as_ref(),
                &workload,
                &workspace,
                options.heartbeat,
                &mut stream,
            )? {
                End::Done => match run_gates(&manifest, &workload, &workspace, &mut stream)? {
                    Ok(()) => (Outcome::Completed, String::new()),
                    Err(reason) => (Outcome::Failed, reason),
                },
                End::Failed(reason) => (Outcome::Failed, reason),
                End::TimedOut(reason) => (Outcome::TimedOut, reason),
            }
        }
        Err(reason) => (Outcome::Failed, reason),
    };

    let record = stream.finish(&workload, outcome, &reason, executor_version)?;
    write_record(&options.record, &record)
}

/// Make the workspace and run the manifest's bootstrap steps in it.
///
/// Returns why the workload cannot go on, as the reason it ends failed.
fn prepare<W: Write>(
    options: &Options,
    workload: &Workload,
    workspace: &Path,
    stream: &mut Stream<'_, W>,
) -> Result<Manifest, String> {
    clone(options, workload, workspace)?;
    let commit = workload
        .repository
        .as_ref()
        .map(|r| r.base_commit.clone())
        .unwrap_or_default();
    stream
        .emit(Kind::WorkspaceReady(WorkspaceReady {
            path: workspace.display().to_string(),
            commit,
        }))
        .map_err(|e| format!("the event could not be written: {e}"))?;

    let manifest_file = workspace.join(workload::manifest_path(workload));
    let text = fs::read_to_string(&manifest_file).map_err(|e| {
        format!(
            "the manifest {} could not be read: {e}",
            manifest_file.display()
        )
    })?;
    let manifest =
        manifest::parse(&text).map_err(|e| format!("the manifest is not valid: {e:?}"))?;

    for (index, step) in manifest.bootstrap.iter().enumerate() {
        bootstrap(&manifest, index + 1, step, workspace)?;
    }
    Ok(manifest)
}

/// Where the environment is read from, for what a gate needs and whether the
/// toolchain is already entered.
fn environment(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Run every gate the manifest declares in the workspace, in name order, and
/// say what each did.
///
/// Every gate runs even if an earlier one failed, so a failure shows all that is
/// wrong at once. The outer error is a failure to write an event. The inner
/// result is the workload's: the reasons it failed, if any gate did.
fn run_gates<W: Write>(
    manifest: &Manifest,
    workload: &Workload,
    workspace: &Path,
    stream: &mut Stream<'_, W>,
) -> Result<Result<(), String>, Refused> {
    let base = &workload
        .repository
        .as_ref()
        .expect("checked when read")
        .base_commit;
    let changed = match changed_files(workspace, base) {
        Ok(changed) => changed,
        Err(reason) => return Ok(Err(reason)),
    };

    let mut failures = Vec::new();
    for name in manifest.gates.keys() {
        stream.emit(Kind::GateStarted(GateStarted { name: name.clone() }))?;
        match gate::run_named(manifest, name, &changed, workspace, environment, say) {
            Ok(result) => {
                if matches!(
                    result.outcome,
                    gate::Outcome::Failed | gate::Outcome::TimedOut
                ) {
                    failures.push(format!("gate {name} {}", result.outcome));
                }
                let value = serde_json::to_value(&result).map_err(|e| Refused(e.to_string()))?;
                let result = serde_json::from_value(value).map_err(|e| Refused(e.to_string()))?;
                stream.emit(Kind::GateFinished(GateFinished {
                    name: name.clone(),
                    result: Some(result),
                }))?;
            }
            Err(error) => failures.push(format!("gate {name} could not be run: {error}")),
        }
    }
    Ok(if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    })
}

/// The files the workspace differs from the base commit by: those changed and
/// those new, and not those git ignores.
fn changed_files(workspace: &Path, base: &str) -> Result<Vec<String>, String> {
    let mut changed = Vec::new();
    for args in [
        vec!["diff", "--name-only", "-z", base],
        vec!["ls-files", "--others", "--exclude-standard", "-z"],
    ] {
        let mut command = vec!["git".to_owned()];
        command.extend(args.iter().map(|a| (*a).to_owned()));
        match execute(&command, workspace, CLONE_TIMEOUT, true, say) {
            Ok((Ended::Exited(0), Some(bytes))) => changed.extend(
                bytes
                    .split(|b| *b == 0)
                    .filter(|name| !name.is_empty())
                    .map(|name| String::from_utf8_lossy(name).into_owned()),
            ),
            _ => return Err("could not find what the agent changed in the workspace".to_owned()),
        }
    }
    Ok(changed)
}

fn clone(options: &Options, workload: &Workload, workspace: &Path) -> Result<(), String> {
    let repository = workload.repository.as_ref().expect("checked when read");
    let source = options.source.clone().unwrap_or_else(|| {
        format!(
            "https://github.com/{}/{}.git",
            repository.owner, repository.name
        )
    });

    fs::create_dir_all(&options.work_dir)
        .map_err(|e| format!("could not create {}: {e}", options.work_dir.display()))?;
    let target = workspace.display().to_string();
    git(
        &["clone", "--no-checkout", "--", &source, &target],
        &options.work_dir,
        "could not clone",
    )?;
    git(
        &[
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--detach",
            &repository.base_commit,
        ],
        workspace,
        "could not check out the base commit in the clone",
    )
}

/// Where what a command says about itself goes: standard error, because
/// standard output carries only events.
pub fn say(message: &str) {
    eprintln!("{message}");
}

fn git(args: &[&str], directory: &Path, what: &str) -> Result<(), String> {
    let mut command = vec!["git".to_owned()];
    command.extend(args.iter().map(|a| (*a).to_owned()));
    match execute(&command, directory, CLONE_TIMEOUT, false, say) {
        Ok((Ended::Exited(0), _)) => Ok(()),
        Ok((Ended::Exited(code), _)) => Err(format!("{what}: git exited with code {code}")),
        Ok((Ended::TimedOut, _)) => Err(format!("{what}: git timed out")),
        Ok((Ended::NotRun(_), _)) => Err(format!("{what}: git could not be started")),
        Err(error) => Err(format!("{what}: {error}")),
    }
}

fn bootstrap(
    manifest: &Manifest,
    number: usize,
    step: &BootstrapStep,
    workspace: &Path,
) -> Result<(), String> {
    let directory: PathBuf = match &step.root {
        Some(root) => workspace.join(root),
        None => workspace.to_path_buf(),
    };
    let described = format!("bootstrap step {number} (`{}`)", step.run.join(" "));
    match execute(
        &gate::wrap(manifest, &step.run, environment),
        &directory,
        Duration::from_secs(step.timeout_seconds),
        false,
        say,
    ) {
        Ok((Ended::Exited(0), _)) => Ok(()),
        Ok((Ended::Exited(code), _)) => Err(format!("{described} exited with code {code}")),
        Ok((Ended::TimedOut, _)) => Err(format!(
            "{described} timed out after {} seconds",
            step.timeout_seconds
        )),
        Ok((Ended::NotRun(_), _)) => Err(format!("{described} could not be started")),
        Err(error) => Err(format!("{described}: {error}")),
    }
}

fn write_record(path: &Path, record: &RunRecord) -> Result<(), Refused> {
    let text = serde_json::to_string_pretty(record).map_err(|e| Refused(e.to_string()))?;
    fs::write(path, text + "\n").map_err(|e| {
        Refused(format!(
            "the record could not be written to {}: {e}",
            path.display()
        ))
    })
}

/// The adapter the options select. A transcript is checked before anything
/// runs, and played by printing it.
fn adapter_for(options: &Options) -> Result<Box<dyn Adapter>, Refused> {
    if let Some(path) = &options.transcript {
        let text =
            fs::read_to_string(path).map_err(|e| Refused(format!("{}: {e}", path.display())))?;
        transcript::check(&text).map_err(|e| Refused(format!("{}: {e}", path.display())))?;
        return Ok(Box::new(Stub {
            command: vec!["cat".to_owned(), path.display().to_string()],
            env_names: vec!["PATH".to_owned()],
        }));
    }
    match &options.agent_command {
        Some(command) => Ok(Box::new(Stub {
            command: command.clone(),
            env_names: options.agent_env.clone(),
        })),
        None => Err(Refused(
            "no agent is selected: pass --transcript or --agent-command (no agent runtime has an adapter yet)"
                .to_owned(),
        )),
    }
}
