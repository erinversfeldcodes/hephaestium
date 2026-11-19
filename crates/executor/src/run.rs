//! Running one workload: prepare a workspace, play the agent, say how it ended.

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use contracts::v1alpha1::agent_runtime::Kind as AgentKind;
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
use crate::claude_code::ClaudeCode;
use crate::cli::Options;
use crate::deliver;
use crate::review;
use crate::secrets::{self, Secrets};
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
pub fn run(
    options: &Options,
    executor_version: &str,
    secrets: &Secrets,
    out: &mut impl Write,
) -> Result<(), Refused> {
    let options = &resolved(options)?;
    let text = fs::read_to_string(&options.workload)
        .map_err(|e| Refused(format!("{}: {e}", options.workload.display())))?;
    let workload = workload::read(&text)
        .map_err(|e| Refused(format!("{}: {e}", options.workload.display())))?;

    let adapter = adapter_for(options, &workload)?;

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

    let mut verdict = None;
    let (outcome, reason) = match prepare(options, &workload, &workspace, &mut stream) {
        Ok(manifest) => {
            // A review is held to changing nothing, so what the workspace is before
            // the agent runs is kept to compare with what it is after.
            let before = if workload.review.is_some() {
                match review::snapshot(&workspace) {
                    Ok(before) => Some(before),
                    Err(reason) => {
                        return finish_failed(
                            stream,
                            &workload,
                            &reason,
                            options,
                            executor_version,
                        );
                    }
                }
            } else {
                None
            };
            match agent::run(
                adapter.as_ref(),
                &workload,
                &workspace,
                options.heartbeat,
                secrets,
                &mut stream,
            )? {
                End::Done if workload.review.is_some() => {
                    match reviewed(&workload, &workspace, before.as_ref()) {
                        Ok(said) => {
                            verdict = Some(said);
                            (Outcome::Completed, String::new())
                        }
                        Err(reason) => (Outcome::Failed, reason),
                    }
                }
                End::Done => {
                    match run_gates(&manifest, &workload, &workspace, secrets, &mut stream)? {
                        Ok(()) => match (options.deliver, secrets.get(secrets::GITHUB_TOKEN)) {
                            (true, Some(token)) => {
                                match deliver::deliver(&workload, &workspace, &options.gh, token) {
                                    Ok(pull_request) => {
                                        stream.emit(Kind::PullRequestOpened(pull_request))?;
                                        (Outcome::Completed, String::new())
                                    }
                                    Err(reason) => (Outcome::Failed, reason),
                                }
                            }
                            _ => (Outcome::Completed, String::new()),
                        },
                        Err(reason) => (Outcome::Failed, reason),
                    }
                }
                End::Failed(reason) => (Outcome::Failed, reason),
                End::TimedOut(reason) => (Outcome::TimedOut, reason),
                End::Cancelled => (Outcome::Failed, String::new()),
            }
        }
        Err(reason) => (Outcome::Failed, reason),
    };

    // Told to stop while it was still working: whatever else went wrong in the
    // stopping, this is how it ended. A workload that had already completed
    // stays completed.
    let (outcome, reason) = if heph::process::cancelled() && outcome != Outcome::Completed {
        let reason = if reason.is_empty() {
            "cancelled by a signal".to_owned()
        } else {
            format!("cancelled by a signal: {reason}")
        };
        (Outcome::Cancelled, reason)
    } else {
        (outcome, reason)
    };

    let mut record = stream.finish(&workload, outcome, &reason, executor_version)?;
    if let Some(verdict) = verdict {
        record.review = Some(serde_json::from_value(verdict).map_err(|e| Refused(e.to_string()))?);
    }
    write_record(&options.record, &record)
}

/// What a review has to show for itself once the agent is done: the workspace is
/// what it was, and the verdict is one that can be kept.
fn reviewed(
    workload: &Workload,
    workspace: &Path,
    before: Option<&review::Snapshot>,
) -> Result<serde_json::Value, String> {
    let review = workload.review.as_ref().expect("checked by the caller");
    let commit = &workload
        .repository
        .as_ref()
        .expect("checked when read")
        .base_commit;
    if let Some(before) = before {
        let after = review::snapshot(workspace)?;
        if let Some(changed) = review::changed_since(before, &after) {
            return Err(changed);
        }
    }
    review::verdict(workspace, commit, &review.lens)
}

/// End a workload that failed before its agent could run, writing its record.
fn finish_failed(
    stream: Stream<'_, impl Write>,
    workload: &Workload,
    reason: &str,
    options: &Options,
    executor_version: &str,
) -> Result<(), Refused> {
    let record = stream.finish(workload, Outcome::Failed, reason, executor_version)?;
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
    secrets: &Secrets,
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
        if heph::process::cancelled() {
            break;
        }
        stream.emit(Kind::GateStarted(GateStarted { name: name.clone() }))?;
        // A gate's secret is the one the executor holds, or else the environment's.
        let lookup = |variable: &str| {
            secrets
                .gate(variable)
                .map(str::to_owned)
                .or_else(|| environment(variable))
        };
        match gate::run_named(manifest, name, Some(&changed), workspace, lookup, say) {
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
            Err(gate::NotRun::Could(gate::CouldNotRun::Cancelled)) => break,
            Err(error) => failures.push(format!("gate {name} could not be run: {error}")),
        }
    }
    if heph::process::cancelled() {
        failures.push("cancelled while the gates ran".to_owned());
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
    )?;

    // Where the agent leaves its commit message is never a change.
    let exclude = workspace.join(".git/info/exclude");
    let mut text = fs::read_to_string(&exclude).unwrap_or_default();
    text.push('\n');
    text.push_str(deliver::MESSAGE_FILE);
    text.push('\n');
    text.push_str(review::VERDICT_FILE);
    text.push('\n');
    fs::write(&exclude, text)
        .map_err(|e| format!("could not keep the commit message out of the changes: {e}"))
}

/// Where what a command says about itself goes: standard error, because
/// standard output carries only events.
pub fn say(message: &str) {
    eprintln!("{message}");
}

fn git(args: &[&str], directory: &Path, what: &str) -> Result<(), String> {
    let mut command = vec!["git".to_owned()];
    command.extend(args.iter().map(|a| (*a).to_owned()));
    let said = RefCell::new(String::new());
    match execute(&command, directory, CLONE_TIMEOUT, false, |message| {
        say(message);
        message.clone_into(&mut said.borrow_mut());
    }) {
        Ok((Ended::Exited(0), _)) => Ok(()),
        Ok((Ended::Exited(code), _)) => Err(format!("{what}: git exited with code {code}")),
        Ok((Ended::TimedOut, _)) => Err(format!("{what}: git timed out")),
        Ok((Ended::Cancelled, _)) => Err(format!("{what}: cancelled")),
        Ok((Ended::NotRun(_), _)) => Err(format!(
            "{what}: git could not be started: {}",
            said.into_inner()
        )),
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
    let said = RefCell::new(String::new());
    match execute(
        &gate::wrap(manifest, &step.run, environment),
        &directory,
        Duration::from_secs(step.timeout_seconds),
        false,
        |message| {
            say(message);
            message.clone_into(&mut said.borrow_mut());
        },
    ) {
        Ok((Ended::Exited(0), _)) => Ok(()),
        Ok((Ended::Exited(code), _)) => Err(format!("{described} exited with code {code}")),
        Ok((Ended::TimedOut, _)) => Err(format!(
            "{described} timed out after {} seconds",
            step.timeout_seconds
        )),
        Ok((Ended::Cancelled, _)) => Err(format!("{described} was cancelled")),
        Ok((Ended::NotRun(_), _)) => Err(format!(
            "{described} could not be started: {}",
            said.into_inner()
        )),
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

/// The options with every path made absolute, once. A relative path means
/// relative to where the executor was started, and every command the executor
/// runs, from the work directory or from the workspace, must mean the same by it.
fn resolved(options: &Options) -> Result<Options, Refused> {
    let absolute = |path: &Path| {
        std::path::absolute(path).map_err(|e| Refused(format!("{}: {e}", path.display())))
    };
    let mut resolved = options.clone();
    resolved.work_dir = absolute(&options.work_dir)?;
    resolved.transcript = options.transcript.as_deref().map(absolute).transpose()?;
    if let Some(source) = &options.source
        && Path::new(source).exists()
    {
        resolved.source = Some(absolute(Path::new(source))?.display().to_string());
    }
    Ok(resolved)
}

/// The adapter the options select. A transcript is checked before anything
/// runs, and played by printing it.
fn adapter_for(options: &Options, workload: &Workload) -> Result<Box<dyn Adapter>, Refused> {
    if let Some(path) = &options.transcript {
        let text =
            fs::read_to_string(path).map_err(|e| Refused(format!("{}: {e}", path.display())))?;
        transcript::check(&text).map_err(|e| Refused(format!("{}: {e}", path.display())))?;
        return Ok(Box::new(Stub {
            command: vec!["cat".to_owned(), path.display().to_string()],
            env_names: vec!["PATH".to_owned()],
        }));
    }
    if let Some(command) = &options.agent_command {
        return Ok(Box::new(Stub {
            command: command.clone(),
            env_names: options.agent_env.clone(),
        }));
    }
    let kind = workload.runtime.as_ref().map_or(0, |runtime| runtime.kind);
    match AgentKind::try_from(kind) {
        Ok(AgentKind::ClaudeCode) => Ok(Box::new(ClaudeCode)),
        Ok(other) => Err(Refused(format!(
            "no adapter exists yet for the {other:?} runtime: pass --transcript or --agent-command for development"
        ))),
        Err(_) => Err(Refused("the workload names no agent runtime".to_owned())),
    }
}
