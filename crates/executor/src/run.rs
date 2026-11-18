//! Running one workload: prepare a workspace, play the agent, say how it ended.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::{RunRecord, Started, Workload, WorkspaceReady};
use heph::manifest::{self, BootstrapStep};
use heph::process::{Ended, execute};

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

    let played = match &options.transcript {
        Some(path) => {
            let text = fs::read_to_string(path)
                .map_err(|e| Refused(format!("{}: {e}", path.display())))?;
            transcript::read(&text).map_err(|e| Refused(format!("{}: {e}", path.display())))?
        }
        None => Vec::new(),
    };

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

    let outcome = prepare(options, &workload, &workspace, &mut stream);
    let (outcome, reason) = match outcome {
        Ok(()) => {
            for kind in played {
                stream.emit(kind)?;
            }
            (Outcome::Completed, String::new())
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
) -> Result<(), String> {
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
        bootstrap(index + 1, step, workspace)?;
    }
    Ok(())
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
fn say(message: &str) {
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

fn bootstrap(number: usize, step: &BootstrapStep, workspace: &Path) -> Result<(), String> {
    let directory: PathBuf = match &step.root {
        Some(root) => workspace.join(root),
        None => workspace.to_path_buf(),
    };
    let described = format!("bootstrap step {number} (`{}`)", step.run.join(" "));
    match execute(
        &step.run,
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
