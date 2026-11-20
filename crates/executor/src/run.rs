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
use crate::completeness;
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

    let mut verdict: Option<Said> = None;
    let (outcome, reason) = match prepare(options, &workload, &workspace, &mut stream) {
        Ok((manifest, since)) => {
            // A review or an evaluation is held to changing nothing, so what the
            // workspace is before the agent runs is kept to compare with what it is after.
            let before = if read_only(&workload) {
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
                End::Done if read_only(&workload) => {
                    match judged(&workload, &workspace, before.as_ref()) {
                        Ok(said) => {
                            verdict = Some(said);
                            (Outcome::Completed, String::new())
                        }
                        Err(reason) => (Outcome::Failed, reason),
                    }
                }
                End::Done => 'done: {
                    // A first run that is about to open a pull request is carried onto
                    // the base branch as it is now, before its gates judge it.
                    let delivering =
                        options.deliver && secrets.get(secrets::GITHUB_TOKEN).is_some();
                    let since = match carry_if(delivering, &workload, &workspace, since) {
                        Ok(since) => since,
                        Err(reason) => break 'done (Outcome::Failed, reason),
                    };
                    match run_gates(&manifest, &workspace, &since, secrets, &mut stream)? {
                        Ok(deferred) => deliver_if_asked(
                            options,
                            secrets,
                            &workload,
                            &workspace,
                            &deferred,
                            &mut stream,
                        )?,
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
    keep(&mut record, verdict)?;
    write_record(&options.record, &record)
}

/// Put the verdict that was kept, if there is one, in the record.
fn keep(record: &mut RunRecord, verdict: Option<Said>) -> Result<(), Refused> {
    match verdict {
        Some(Said::Review(said)) => {
            record.review = Some(serde_json::from_value(said).map_err(|e| Refused(e.to_string()))?);
        }
        Some(Said::Completeness(said)) => {
            record.completeness =
                Some(serde_json::from_value(said).map_err(|e| Refused(e.to_string()))?);
        }
        None => {}
    }
    Ok(())
}

/// A verdict that was kept, and for which kind of read-only run.
enum Said {
    Review(serde_json::Value),
    Completeness(serde_json::Value),
}

/// Whether the workload is held to changing nothing and ends with a verdict.
fn read_only(workload: &Workload) -> bool {
    workload.review.is_some() || workload.completeness.is_some()
}

/// What a read-only run has to show for itself once the agent is done: the workspace is
/// what it was, and the verdict is one that can be kept.
fn judged(
    workload: &Workload,
    workspace: &Path,
    before: Option<&review::Snapshot>,
) -> Result<Said, String> {
    if workload.completeness.is_some() {
        evaluated(workload, workspace, before).map(Said::Completeness)
    } else {
        reviewed(workload, workspace, before).map(Said::Review)
    }
}

/// What an evaluation has to show: the workspace is what it was, and the verdict names
/// the ticket and the commit it was asked about and passes the completeness contract.
fn evaluated(
    workload: &Workload,
    workspace: &Path,
    before: Option<&review::Snapshot>,
) -> Result<serde_json::Value, String> {
    let commit = &workload
        .repository
        .as_ref()
        .expect("checked when read")
        .base_commit;
    let ticket = workload.ticket.as_ref().map_or(0, |t| t.number);
    if let Some(before) = before {
        let after = review::snapshot(workspace)?;
        if let Some(changed) = review::changed_since_by("evaluation", before, &after) {
            return Err(changed);
        }
    }
    completeness::verdict(workspace, commit, ticket)
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
) -> Result<(Manifest, String), String> {
    clone(options, workload, workspace)?;
    bring_in_base(workload, workspace)?;
    let commit = git_output(&["rev-parse", "HEAD"], workspace)?
        .trim()
        .to_owned();
    stream
        .emit(Kind::WorkspaceReady(WorkspaceReady {
            path: workspace.display().to_string(),
            commit: commit.clone(),
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
    Ok((manifest, commit))
}

/// A revision works on the branch as it is and on the base branch as it is now,
/// so what the base gained since the pull request was opened is merged in before
/// the agent starts. It is a merge commit, never a rebase, so the push that
/// delivers the revision is still a plain fast-forward of the branch.
///
/// A conflict ends the run with the files named, and the branch is untouched.
fn bring_in_base(workload: &Workload, workspace: &Path) -> Result<(), String> {
    let Some(revision) = workload.revision.as_ref() else {
        return Ok(());
    };
    let base = &workload
        .repository
        .as_ref()
        .expect("checked when read")
        .base_branch;
    let tip = format!("origin/{base}");
    // The clone is fresh, so the base branch's tip is as it is now.
    if git(
        &["merge-base", "--is-ancestor", &tip, "HEAD"],
        workspace,
        "could not compare the branch with the base branch",
    )
    .is_ok()
    {
        return Ok(());
    }
    let message = format!("Merge branch '{base}' into {}", revision.branch);
    if git(
        &["merge", "--no-ff", "--no-edit", "-m", &message, &tip],
        workspace,
        "could not merge the base branch",
    )
    .is_ok()
    {
        return Ok(());
    }
    let files = git_output(&["diff", "--name-only", "--diff-filter=U"], workspace)
        .unwrap_or_default()
        .lines()
        .collect::<Vec<_>>()
        .join(", ");
    let _ = git(
        &["merge", "--abort"],
        workspace,
        "could not abort the merge",
    );
    Err(format!(
        "the branch conflicts with the base branch '{base}' in {}",
        if files.is_empty() {
            "files that could not be listed"
        } else {
            &files
        }
    ))
}

/// Open the pull request when the run was asked to and holds the token to, and say how
/// the workload ended. The outer error is a failure to write an event.
fn deliver_if_asked<W: Write>(
    options: &Options,
    secrets: &Secrets,
    workload: &Workload,
    workspace: &Path,
    deferred: &[deliver::Deferred],
    stream: &mut Stream<'_, W>,
) -> Result<(Outcome, String), Refused> {
    let (true, Some(token)) = (options.deliver, secrets.get(secrets::GITHUB_TOKEN)) else {
        return Ok((Outcome::Completed, String::new()));
    };
    match deliver::deliver(workload, workspace, &options.gh, token, deferred) {
        Ok(pull_request) => {
            stream.emit(Kind::PullRequestOpened(pull_request))?;
            Ok((Outcome::Completed, String::new()))
        }
        Err(reason) => Ok((Outcome::Failed, reason)),
    }
}

/// Carry the work onto the base branch when it is about to be delivered, and leave it
/// where it is otherwise: a run that opens no pull request has no ground to be on.
fn carry_if(
    delivering: bool,
    workload: &Workload,
    workspace: &Path,
    since: String,
) -> Result<String, String> {
    if delivering {
        carry_onto_base(workload, workspace, &since)
    } else {
        Ok(since)
    }
}

/// A first run can take minutes, and the base branch can move in that time. Before its
/// pull request is opened, the work is carried onto the base branch's current tip, so
/// the pull request is not opened against ground that has changed.
///
/// Work that touches nothing the base branch touched is carried over mechanically, with
/// no commit of its own: the checkout moves to the tip and the uncommitted work goes
/// with it, so the pull request has one commit on current ground. Work that touches
/// a file the base also touched is not carried over, and the run ends with the files
/// named. Returns the commit the work now stands on, which what the agent changed is
/// judged from.
fn carry_onto_base(workload: &Workload, workspace: &Path, since: &str) -> Result<String, String> {
    if workload.revision.is_some() || workload.review.is_some() {
        return Ok(since.to_owned());
    }
    let base = &workload
        .repository
        .as_ref()
        .expect("checked when read")
        .base_branch;
    git(
        &["fetch", "--quiet", "origin", base],
        workspace,
        "could not fetch the base branch",
    )?;
    let tip = git_output(&["rev-parse", "FETCH_HEAD"], workspace)?
        .trim()
        .to_owned();
    if tip == since {
        return Ok(tip);
    }
    let ours = changed_files(workspace, since)?;
    if ours.is_empty() {
        // Nothing to carry: delivery says the agent changed nothing.
        return Ok(since.to_owned());
    }
    let range = format!("{since}..{tip}");
    let gained = git_output(&["diff", "--name-only", &range], workspace)?;
    let overlap: Vec<&str> = gained
        .lines()
        .filter(|file| ours.iter().any(|mine| mine == file))
        .collect();
    if !overlap.is_empty() {
        return Err(format!(
            "the base branch '{base}' changed {} too since this run began, so the change cannot be carried onto it without a person or the agent resolving the difference",
            overlap.join(", ")
        ));
    }
    git(
        &["stash", "push", "--quiet", "--include-untracked"],
        workspace,
        "could not set the work aside to move it onto the base branch",
    )?;
    git(
        &[
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--quiet",
            "--detach",
            &tip,
        ],
        workspace,
        "could not check out the base branch's tip",
    )?;
    git(
        &["stash", "pop", "--quiet"],
        workspace,
        "could not put the work back on the base branch's tip",
    )?;
    Ok(tip)
}

/// What a git command wrote to standard output.
fn git_output(args: &[&str], directory: &Path) -> Result<String, String> {
    let mut command = vec!["git".to_owned()];
    command.extend(args.iter().map(|a| (*a).to_owned()));
    match execute(&command, directory, CLONE_TIMEOUT, true, say) {
        Ok((Ended::Exited(0), Some(bytes))) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        _ => Err(format!("git {} failed", args.join(" "))),
    }
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
    workspace: &Path,
    since: &str,
    secrets: &Secrets,
    stream: &mut Stream<'_, W>,
) -> Result<Result<Vec<deliver::Deferred>, String>, Refused> {
    let changed = match changed_files(workspace, since) {
        Ok(changed) => changed,
        Err(reason) => return Ok(Err(reason)),
    };

    let mut failures = Vec::new();
    let mut deferred = Vec::new();
    for (name, declared) in &manifest.gates {
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
        let command = gate::command_for(manifest, declared, lookup);
        let outcome = match gate::run_named(manifest, name, Some(&changed), workspace, lookup, say)
        {
            // A gate the workspace could not run, that the repository says a check on
            // the pull request runs too, is left to that check.
            Err(gate::NotRun::Missing(missing)) => gate::deferred(
                name,
                declared,
                command,
                0,
                format!(
                    "what it needs is not set: {}",
                    gate::describe_missing(&missing)
                ),
            )
            .map_or_else(|| Err(gate::NotRun::Missing(missing)), Ok),
            Ok(result) if result.outcome == gate::Outcome::TimedOut => gate::deferred(
                name,
                declared,
                result.command.clone(),
                result.duration_ms,
                format!("timed out after {}s", declared.timeout_seconds),
            )
            .map_or(Ok(result), Ok),
            other => other,
        };
        match outcome {
            Ok(result) => {
                if matches!(
                    result.outcome,
                    gate::Outcome::Failed | gate::Outcome::TimedOut
                ) {
                    failures.push(format!("gate {name} {}", result.outcome));
                }
                if let (gate::Outcome::Deferred, Some(check), Some(reason)) =
                    (result.outcome, &result.ci_check, &result.reason)
                {
                    deferred.push(deliver::Deferred {
                        gate: name.clone(),
                        reason: reason.clone(),
                        ci_check: check.clone(),
                    });
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
        Ok(deferred)
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
    text.push_str(deliver::RESOLUTION_FILE);
    text.push('\n');
    text.push_str(review::VERDICT_FILE);
    text.push('\n');
    text.push_str(completeness::VERDICT_FILE);
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
