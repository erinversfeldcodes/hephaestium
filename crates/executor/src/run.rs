//! Running one workload: prepare a workspace, play the agent, say how it ended.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use contracts::v1alpha1::agent_runtime::Kind as AgentKind;
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::step::Kind as StepKind;
use contracts::v1alpha1::{
    BaseMoved, CostTotal, GateFinished, GateStarted, Heartbeat, Lens, Revision, RunRecord, Started,
    Step, StepFinished, StepStarted, TokenBreakdown, Workload, WorkspaceReady,
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
use crate::gap;
use crate::prepush::{self, Reading};
use crate::redact::Redactor;
use crate::review;
use crate::secrets::{self, Secrets};
use crate::stream::{Spent, Stream};
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

    let redactor = secrets.redactor();
    let mut stream = Stream::new(out, &workload.id).redacting(redactor.clone());
    stream.emit(Kind::Started(Started {
        executor_version: executor_version.to_owned(),
    }))?;

    let mut verdict: Option<Said> = None;
    // What the workload has spent on every agent it has run, so that asking the agent a
    // second time spends from the budget the first left and not from a new one.
    let spent = Cell::new(0.0_f64);
    // Which agents the run started and what each spent, when it reads its own change.
    let steps = RefCell::new(Vec::<Step>::new());
    let (outcome, reason) = match prepare(options, &workload, &workspace, secrets, &mut stream) {
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
                            &redactor,
                        );
                    }
                }
            } else {
                None
            };
            let ended = run_first_agent(
                adapter.as_ref(),
                &workload,
                &workspace,
                options,
                secrets,
                &spent,
                &mut stream,
            )?;
            match ended {
                End::Done if read_only(&workload) => {
                    match judged(&workload, &workspace, before.as_ref()) {
                        Ok(said) => {
                            verdict = Some(said);
                            (Outcome::Completed, String::new())
                        }
                        Err(reason) => (Outcome::Failed, reason),
                    }
                }
                End::Done => {
                    let asking = Asking {
                        adapter: adapter.as_ref(),
                        options,
                        secrets,
                        spent: &spent,
                        steps: &steps,
                    };
                    finish_first_run(
                        &asking,
                        &workload,
                        &workspace,
                        &manifest,
                        since,
                        &mut stream,
                    )?
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
    record.steps = steps.into_inner();
    keep(&mut record, verdict)?;
    write_record(&options.record, &record, &redactor)
}

/// Run the agent that makes the change. A run that reads its own change says which agent is
/// running, so this one is told too.
fn run_first_agent<W: Write>(
    adapter: &dyn Adapter,
    workload: &Workload,
    workspace: &Path,
    options: &Options,
    secrets: &Secrets,
    spent: &Cell<f64>,
    stream: &mut Stream<'_, W>,
) -> Result<End, Refused> {
    let told = options.deliver
        && secrets.get(secrets::GITHUB_TOKEN).is_some()
        && reads_before_push(workload);
    if told {
        step_started(stream, StepKind::Implementation, 0, "")?;
    }
    let ended = agent::run(
        adapter,
        workload,
        workspace,
        options.heartbeat,
        secrets,
        spent,
        stream,
    )?;
    if told {
        step_finished(stream, StepKind::Implementation, 0, "", "")?;
    }
    Ok(ended)
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
        Some(Said::Gap(step)) => record.gap = Some(step),
        None => {}
    }
    Ok(())
}

/// A verdict that was kept, and for which kind of read-only run.
enum Said {
    Review(serde_json::Value),
    Completeness(serde_json::Value),
    Gap(contracts::v1alpha1::GapStep),
}

/// Whether the workload is held to changing nothing and ends with a verdict.
fn read_only(workload: &Workload) -> bool {
    workload.review.is_some() || workload.completeness.is_some() || workload.gap.is_some()
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
    } else if workload.gap.is_some() {
        stepped(workload, workspace, before).map(Said::Gap)
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

/// What a step of the questioning has to show: the workspace is what it was, and what it
/// wrote is one of the three ways a step may end.
fn stepped(
    workload: &Workload,
    workspace: &Path,
    before: Option<&review::Snapshot>,
) -> Result<contracts::v1alpha1::GapStep, String> {
    let ticket = workload.ticket.as_ref().map_or(0, |t| t.number);
    if let Some(before) = before {
        let after = review::snapshot(workspace)?;
        if let Some(changed) = review::changed_since_by("step", before, &after) {
            return Err(changed);
        }
    }
    gap::step(workspace, ticket)
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
    redactor: &Redactor,
) -> Result<(), Refused> {
    let record = stream.finish(workload, Outcome::Failed, reason, executor_version)?;
    write_record(&options.record, &record, redactor)
}

/// Make the workspace and run the manifest's bootstrap steps in it.
///
/// Returns why the workload cannot go on, as the reason it ends failed.
fn prepare<W: Write>(
    options: &Options,
    workload: &Workload,
    workspace: &Path,
    secrets: &Secrets,
    stream: &mut Stream<'_, W>,
) -> Result<(Manifest, String), String> {
    clone(options, workload, workspace)?;
    bring_in_base(workload, workspace)?;
    // A run that will push finds out now whether it can, before any agent is paid for.
    if let (true, false, Some(token)) = (
        options.deliver,
        read_only(workload),
        secrets.get(secrets::GITHUB_TOKEN),
    ) {
        deliver::check_push(workload, workspace, &options.gh, token)?;
    }
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

/// What carrying the work onto the base branch came to.
enum Carried {
    /// The work stands on this commit and nothing more is needed.
    Onto(String),
    /// The work and what the base gained are merged in the working tree, and the agent is to
    /// resolve what is left and reconfirm its change.
    Merged(Merge),
}

/// A change merged with what the base gained, left in the working tree for the agent.
struct Merge {
    /// The base branch's tip, which the merged work stands on.
    tip: String,
    /// What came in, one line a commit.
    commits: Vec<String>,
    /// The files both the change and the base changed.
    overlap: Vec<String>,
    /// The files where both changed the same lines, which hold conflict markers.
    conflicted: Vec<String>,
}

/// The workload as the agent is asked about it a second time: its own first change, which no
/// pull request holds yet, and what came in. A revision with no pull request is how the
/// prompt says so.
fn asked_again(workload: &Workload, merge: &Merge) -> Workload {
    let mut asked = workload.clone();
    asked.revision = Some(Revision {
        attempt: 1,
        base_moved: Some(BaseMoved {
            commits: merge.commits.clone(),
            overlapping_files: merge.overlap.clone(),
        }),
        ..Revision::default()
    });
    asked
}

/// What asking the agent a second time needs, besides the workload.
struct Asking<'a> {
    adapter: &'a dyn Adapter,
    options: &'a Options,
    secrets: &'a Secrets,
    spent: &'a Cell<f64>,
    /// The agents the run started, in order, as the run record lists them.
    steps: &'a RefCell<Vec<Step>>,
}

/// What a first run or a revision does once its agent is done: carry the work onto the base
/// branch as it is now, asking the agent to resolve what the base changed, then judge it by
/// the repository's gates and open the pull request.
fn finish_first_run<W: Write>(
    asking: &Asking<'_>,
    workload: &Workload,
    workspace: &Path,
    manifest: &Manifest,
    since: String,
    stream: &mut Stream<'_, W>,
) -> Result<(Outcome, String), Refused> {
    let (options, secrets) = (asking.options, asking.secrets);
    let delivering = options.deliver && secrets.get(secrets::GITHUB_TOKEN).is_some();
    if let Some(question) = stops_to_ask(workload, workspace, delivering, stream) {
        stream.ask(&question);
        return Ok((Outcome::Blocked, blocked_reason(workload)));
    }
    let since = match carry_if(delivering, workload, workspace, since) {
        Ok(Carried::Onto(since)) => since,
        // The base changed what the agent changed: the agent resolves the difference and
        // reconfirms its change, in this run and within its budget, and what it leaves is
        // checked before anything is judged.
        Ok(Carried::Merged(merge)) => {
            match resolve_merge(asking, workload, workspace, &merge, stream)? {
                Ok(since) => since,
                Err(ended) => return Ok(ended),
            }
        }
        Err(reason) => return Ok((Outcome::Failed, reason)),
    };
    // A run that reads before it pushes runs the gates itself, because a gate that fails is
    // handed to the implementing agent to fix, in the same rounds a reader's finding is.
    if delivering && reads_before_push(workload) {
        return read_before_push(asking, workload, workspace, manifest, &since, stream);
    }
    match run_gates(
        manifest,
        workspace,
        &since,
        secrets,
        options.heartbeat,
        stream,
    )? {
        Ok(deferred) => deliver_if_asked(options, secrets, workload, workspace, &deferred, stream),
        Err(failed) => Ok((Outcome::Failed, failed.summary)),
    }
}

/// Why the run ended blocked and not failed.
fn blocked_reason(workload: &Workload) -> String {
    if workload.revision.is_some() {
        "the agent cannot go on without a person's answer".to_owned()
    } else {
        "the agent has nothing to deliver, and a person is asked what to do".to_owned()
    }
}

/// What a person is asked, when the agent's work ended without anything to deliver or with
/// something it could not decide: its own question; that the base already fixes the ticket,
/// which a person closes; or, for a first run that is to deliver and changed nothing, what the
/// agent said about why. None when there is something to carry on with, or nothing to say.
fn stops_to_ask<W: Write>(
    workload: &Workload,
    workspace: &Path,
    delivering: bool,
    stream: &Stream<'_, W>,
) -> Option<String> {
    let account = stream.account();
    if let Ok(text) = fs::read_to_string(workspace.join(deliver::QUESTION_FILE))
        && !text.trim().is_empty()
    {
        return Some(text.trim().to_owned());
    }
    // A revision is judged on what it changed; only a first run is asked about the ticket.
    if workload.revision.is_some() {
        return None;
    }
    if let Some(commit) = deliver::resolved_by(workspace) {
        let mut question = format!(
            "The ticket's problem looks already fixed on the base branch, by commit {commit}, so there is nothing to change. Should a person close the ticket?"
        );
        if let Some(account) = account {
            question.push_str("\n\nThe agent's account:\n\n");
            question.push_str(account);
        }
        return Some(question);
    }
    let unchanged = git_output(&["status", "--porcelain"], workspace).is_ok_and(|s| s.is_empty());
    if delivering && unchanged {
        return account.map(str::to_owned);
    }
    None
}

/// Whether the run reads its own change under lenses before it pushes: it was given lenses and
/// rounds to read them in. A workload that is itself a read is never one that reads.
fn reads_before_push(workload: &Workload) -> bool {
    !workload.lenses.is_empty() && workload.prepush_rounds > 0 && !read_only(workload)
}

/// Say that the agent of a step has started: its events follow, and this is what tells one
/// agent's from another's. The kind is the run record's, whose values the event's mirror.
fn step_started<W: Write>(
    stream: &mut Stream<'_, W>,
    kind: StepKind,
    round: i32,
    lens: &str,
) -> Result<(), Refused> {
    stream.emit(Kind::StepStarted(StepStarted {
        kind: kind as i32,
        round,
        lens: lens.to_owned(),
    }))?;
    Ok(())
}

/// Say that the agent of a step has ended, and for a reading what it ended in.
fn step_finished<W: Write>(
    stream: &mut Stream<'_, W>,
    kind: StepKind,
    round: i32,
    lens: &str,
    verdict: &str,
) -> Result<(), Refused> {
    stream.emit(Kind::StepFinished(StepFinished {
        kind: kind as i32,
        round,
        lens: lens.to_owned(),
        verdict: verdict.to_owned(),
    }))?;
    Ok(())
}

/// What a step spent: what the run had spent after it, less what it had before. The parts of a
/// token cost are taken the same way, and a step whose runtime gave none has none.
fn spent_between(
    before: &BTreeMap<(i32, i32), Spent>,
    after: &BTreeMap<(i32, i32), Spent>,
) -> Vec<CostTotal> {
    after
        .iter()
        .filter_map(|(&(unit, source), now)| {
            let was = before.get(&(unit, source)).cloned().unwrap_or_default();
            let amount = now.amount - was.amount;
            (amount > 0.0).then(|| CostTotal {
                unit,
                amount,
                source,
                breakdown: parts_between(was.breakdown.as_ref(), now.breakdown.as_ref()),
            })
        })
        .collect()
}

/// The parts of tokens a step spent: those after it, less those before. None when no part
/// grew, which is a step whose runtime did not say how its tokens were made up.
fn parts_between(
    before: Option<&TokenBreakdown>,
    after: Option<&TokenBreakdown>,
) -> Option<TokenBreakdown> {
    let now = after?;
    let was = before.copied().unwrap_or_default();
    let spent = TokenBreakdown {
        input: now.input - was.input,
        output: now.output - was.output,
        cache_creation: now.cache_creation - was.cache_creation,
        cache_read: now.cache_read - was.cache_read,
    };
    let grew = [
        spent.input,
        spent.output,
        spent.cache_creation,
        spent.cache_read,
    ]
    .iter()
    .any(|part| *part > 0.0);
    grew.then_some(spent)
}

/// Record a step: the agent that ran, for what, and what it spent.
fn note_step(
    asking: &Asking<'_>,
    kind: StepKind,
    lens: &str,
    round: i32,
    verdict: &str,
    costs: Vec<CostTotal>,
) {
    asking.steps.borrow_mut().push(Step {
        kind: kind as i32,
        lens: lens.to_owned(),
        round,
        verdict: verdict.to_owned(),
        costs,
    });
}

/// The lenses the change needs, from what it changed and the components that own it, out of
/// the library the workload carries.
fn lenses_needed<'a>(
    workload: &'a Workload,
    manifest: &Manifest,
    workspace: &Path,
    since: &str,
) -> Result<Vec<&'a Lens>, String> {
    let changed = changed_files(workspace, since)?;
    let plan = heph::lenses::plan(manifest, &changed).map_err(|error| error.to_string())?;
    let needed: Vec<String> = plan.lenses.iter().map(|lens| lens.pack.clone()).collect();
    Ok(prepush::choose(&workload.lenses, &needed))
}

/// How long to wait before a reader that could not run is started again: an outage of the API
/// is waited out and not spent in three attempts at once. `HEPH_READER_RETRY_WAIT_MS`, ten
/// seconds if it is not set, and a stop is noticed while waiting.
fn wait_before_trying_again() {
    let wait = environment("HEPH_READER_RETRY_WAIT_MS")
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(10_000);
    let deadline = std::time::Instant::now() + Duration::from_millis(wait);
    while std::time::Instant::now() < deadline && !heph::process::cancelled() {
        std::thread::sleep(Duration::from_millis(100).min(Duration::from_millis(wait)));
    }
}

/// What the run says when the rounds were used up and a lens that blocks had not approved.
fn not_pushed(unsatisfied: &[(String, prepush::Outcome)], rounds: i32) -> String {
    let mut details = Vec::new();
    for (pack, outcome) in unsatisfied {
        match outcome {
            prepush::Outcome::Revise(findings) => {
                let titles: Vec<&str> = findings.iter().map(|f| f.title.as_str()).collect();
                details.push(format!("lens {pack} found: {}", titles.join("; ")));
            }
            prepush::Outcome::Unread(reason) => {
                details.push(format!("lens {pack} gave no approval: {reason}"));
            }
            prepush::Outcome::Approved => {}
        }
    }
    format!(
        "the change was read under its lenses for {rounds} round(s) and a lens that blocks did not approve it, so: {}. Nothing was pushed, and the workspace is kept.",
        details.join("; ")
    )
}

/// Read the change under every lens it needs before it is pushed, and push it only when each
/// lens that blocks has approved the commit about to be pushed. What blocks goes back to the
/// implementing agent, the gates run again on what it changed, and the commit is rewritten, in
/// as many rounds as the workload allows. A change that is still blocked when they are used
/// up is not pushed.
#[allow(
    clippy::too_many_lines,
    reason = "one loop, read top to bottom: read, decide, revise, gate, amend, push"
)]
fn read_before_push<W: Write>(
    asking: &Asking<'_>,
    workload: &Workload,
    workspace: &Path,
    manifest: &Manifest,
    since: &str,
    stream: &mut Stream<'_, W>,
) -> Result<(Outcome, String), Refused> {
    let (options, secrets) = (asking.options, asking.secrets);
    let Some(token) = secrets.get(secrets::GITHUB_TOKEN) else {
        return Ok((Outcome::Failed, "no token to push with".to_owned()));
    };
    // Everything spent so far is the implementation's.
    let mut marked = stream.costs().clone();
    note_step(
        asking,
        StepKind::Implementation,
        "",
        0,
        "",
        spent_between(&BTreeMap::new(), &marked),
    );

    // The gates come first, and a gate that fails is not the end of the run: the agent is
    // resumed, told what the gate printed as data, and the gates run again, for as many turns
    // as there are rounds.
    let mut fixes = 0;
    let mut deferred = loop {
        match run_gates(
            manifest,
            workspace,
            since,
            secrets,
            options.heartbeat,
            stream,
        )? {
            Ok(deferred) => break deferred,
            Err(failed) => {
                if fixes >= workload.prepush_rounds || heph::process::cancelled() {
                    return Ok((Outcome::Failed, failed.with_output()));
                }
                fixes += 1;
                let asked = prepush::as_revision(workload, fixes, failed.checks);
                step_started(stream, StepKind::Revision, fixes, "")?;
                let ended = agent::run(
                    asking.adapter,
                    &asked,
                    workspace,
                    options.heartbeat,
                    secrets,
                    asking.spent,
                    stream,
                )?;
                step_finished(stream, StepKind::Revision, fixes, "", "")?;
                match ended {
                    End::Done => {}
                    End::Failed(reason) => return Ok((Outcome::Failed, reason)),
                    End::TimedOut(reason) => return Ok((Outcome::TimedOut, reason)),
                    End::Cancelled => return Ok((Outcome::Failed, String::new())),
                }
                let now = stream.costs().clone();
                note_step(
                    asking,
                    StepKind::Revision,
                    "",
                    fixes,
                    "",
                    spent_between(&marked, &now),
                );
                marked = now;
            }
        }
    };

    let committed = match deliver::commit_locally(workload, workspace) {
        Ok(committed) => committed,
        Err(reason) => return Ok((Outcome::Failed, reason)),
    };

    let reading = Reading {
        adapter: asking.adapter,
        heartbeat: options.heartbeat,
        secrets,
        spent: asking.spent,
    };
    let rounds = workload.prepush_rounds;
    let mut round = 1;
    loop {
        if heph::process::cancelled() {
            return Ok((
                Outcome::Failed,
                "cancelled while the change was being read".to_owned(),
            ));
        }
        let head = match git_output(&["rev-parse", "HEAD"], workspace) {
            Ok(head) => head.trim().to_owned(),
            Err(reason) => return Ok((Outcome::Failed, reason)),
        };
        let chosen = match lenses_needed(workload, manifest, workspace, since) {
            Ok(chosen) => chosen,
            Err(reason) => return Ok((Outcome::Failed, reason)),
        };

        // Every chosen lens reads the commit, each with a fresh agent.
        let mut unsatisfied: Vec<(String, prepush::Outcome)> = Vec::new();
        for lens in chosen {
            // A reader that could not run is tried again in this round, so an outage of the
            // API does not use up a round that is the implementing agent's chance to fix what
            // the readers find. A verdict that was given is the reader's judgement and is not.
            let mut tries = 1;
            let said = loop {
                step_started(stream, StepKind::Review, round, &lens.pack)?;
                let said = prepush::read(&reading, workload, lens, workspace, &head, stream)?;
                let now = stream.costs().clone();
                let verdict = said
                    .as_ref()
                    .ok()
                    .and_then(|value| value["verdict"].as_str())
                    .unwrap_or_default();
                step_finished(stream, StepKind::Review, round, &lens.pack, verdict)?;
                note_step(
                    asking,
                    StepKind::Review,
                    &lens.pack,
                    round,
                    verdict,
                    spent_between(&marked, &now),
                );
                marked = now;
                let again = match &said {
                    Err(reason) => {
                        tries < prepush::TRIES
                            && prepush::retryable(reason)
                            && !heph::process::cancelled()
                    }
                    Ok(_) => false,
                };
                if !again {
                    break said;
                }
                tries += 1;
                wait_before_trying_again();
            };
            // A lens that does not block is read and reported, and never stops the push.
            let outcome = prepush::outcome(&said);
            if prepush::blocks(lens) && !matches!(outcome, prepush::Outcome::Approved) {
                unsatisfied.push((lens.pack.clone(), outcome));
            }
        }
        if unsatisfied.is_empty() {
            break;
        }
        if round >= rounds {
            return Ok((Outcome::Failed, not_pushed(&unsatisfied, rounds)));
        }

        // What blocks goes back to the implementing agent, as data. A lens that could not
        // review has nothing to send back, so it is read again in the next round.
        let findings: Vec<(&str, Vec<prepush::Finding>)> = unsatisfied
            .iter()
            .filter_map(|(pack, outcome)| match outcome {
                prepush::Outcome::Revise(found) => Some((pack.as_str(), found.clone())),
                _ => None,
            })
            .collect();
        if !findings.is_empty() {
            let asked = prepush::as_revision(workload, round, prepush::failures(&findings));
            step_started(stream, StepKind::Revision, round, "")?;
            let ended = agent::run(
                asking.adapter,
                &asked,
                workspace,
                options.heartbeat,
                secrets,
                asking.spent,
                stream,
            )?;
            step_finished(stream, StepKind::Revision, round, "", "")?;
            match ended {
                End::Done => {}
                End::Failed(reason) => return Ok((Outcome::Failed, reason)),
                End::TimedOut(reason) => return Ok((Outcome::TimedOut, reason)),
                End::Cancelled => return Ok((Outcome::Failed, String::new())),
            }
            let now = stream.costs().clone();
            note_step(
                asking,
                StepKind::Revision,
                "",
                round,
                "",
                spent_between(&marked, &now),
            );
            marked = now;
            match run_gates(
                manifest,
                workspace,
                since,
                secrets,
                options.heartbeat,
                stream,
            )? {
                Ok(still_deferred) => deferred = still_deferred,
                Err(failed) => return Ok((Outcome::Failed, failed.with_output())),
            }
            if let Err(reason) = deliver::amend_locally(workload, workspace) {
                return Ok((Outcome::Failed, reason));
            }
        }
        round += 1;
    }

    match deliver::push_and_open(
        workload,
        workspace,
        &options.gh,
        token,
        &deferred,
        &committed,
    ) {
        Ok(pull_request) => {
            stream.emit(Kind::PullRequestOpened(pull_request))?;
            Ok((Outcome::Completed, String::new()))
        }
        Err(reason) => Ok((Outcome::Failed, reason)),
    }
}

/// Ask the agent to resolve what the base changed and reconfirm its change, and check what it
/// left. The inner error is how the run ended, when it did not go on.
fn resolve_merge<W: Write>(
    asking: &Asking<'_>,
    workload: &Workload,
    workspace: &Path,
    merge: &Merge,
    stream: &mut Stream<'_, W>,
) -> Result<Result<String, (Outcome, String)>, Refused> {
    match agent::run(
        asking.adapter,
        &asked_again(workload, merge),
        workspace,
        asking.options.heartbeat,
        asking.secrets,
        asking.spent,
        stream,
    )? {
        End::Done => {}
        End::Failed(reason) => return Ok(Err((Outcome::Failed, reason))),
        End::TimedOut(reason) => return Ok(Err((Outcome::TimedOut, reason))),
        End::Cancelled => return Ok(Err((Outcome::Failed, String::new()))),
    }
    Ok(settle_merge(workspace, merge).map_err(|reason| (Outcome::Failed, reason)))
}

/// Carry the work onto the base branch when it is about to be delivered, and leave it
/// where it is otherwise: a run that opens no pull request has no ground to be on.
fn carry_if(
    delivering: bool,
    workload: &Workload,
    workspace: &Path,
    since: String,
) -> Result<Carried, String> {
    if delivering {
        carry_onto_base(workload, workspace, &since)
    } else {
        Ok(Carried::Onto(since))
    }
}

/// A first run can take minutes, and the base branch can move in that time. Before its
/// pull request is opened, the work is carried onto the base branch's current tip, so
/// the pull request is not opened against ground that has changed.
///
/// Work that touches nothing the base branch touched is carried over mechanically, with
/// no commit of its own: the checkout moves to the tip and the uncommitted work goes
/// with it, so the pull request has one commit on current ground. Work that touches
/// a file the base also touched is merged with it in the working tree, and what is left,
/// conflict markers where both changed the same lines, goes to the agent to resolve.
/// Returns where the work now stands.
fn carry_onto_base(workload: &Workload, workspace: &Path, since: &str) -> Result<Carried, String> {
    if workload.revision.is_some() || workload.review.is_some() {
        return Ok(Carried::Onto(since.to_owned()));
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
        return Ok(Carried::Onto(tip));
    }
    let ours = changed_files(workspace, since)?;
    if ours.is_empty() {
        // Nothing to carry: delivery says the agent changed nothing.
        return Ok(Carried::Onto(since.to_owned()));
    }
    let range = format!("{since}..{tip}");
    let gained = git_output(&["diff", "--name-only", &range], workspace)?;
    let overlap: Vec<String> = gained
        .lines()
        .filter(|file| ours.iter().any(|mine| mine == file))
        .map(str::to_owned)
        .collect();
    if !overlap.is_empty() {
        return merge_with_base(workspace, since, &tip, overlap);
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
    Ok(Carried::Onto(tip))
}

/// Arguments for a git command that makes a commit or a merge, with an identity of its own
/// so that nothing depends on the checkout's configuration.
fn with_identity<'a>(args: &[&'a str]) -> Vec<&'a str> {
    [
        "-c",
        "user.name=Hephaestium",
        "-c",
        "user.email=hephaestium@users.noreply.invalid",
        "-c",
        "commit.gpgsign=false",
    ]
    .into_iter()
    .chain(args.iter().copied())
    .collect()
}

/// Merge the agent's work with what the base gained, in the working tree. The work is
/// committed for a moment, on the commit the run began at, so that git can merge it as it
/// merges anything: the result is left staged with conflict markers where both changed the
/// same lines, and is not committed. That commit is never pushed.
fn merge_with_base(
    workspace: &Path,
    since: &str,
    tip: &str,
    overlap: Vec<String>,
) -> Result<Carried, String> {
    git(
        &["add", "--all"],
        workspace,
        "could not set the work aside to merge it",
    )?;
    git(
        &with_identity(&[
            "commit",
            "--quiet",
            "--no-verify",
            "-m",
            "the agent's change, set aside to be merged",
        ]),
        workspace,
        "could not set the work aside to merge it",
    )?;
    let work = git_output(&["rev-parse", "HEAD"], workspace)?
        .trim()
        .to_owned();
    git(
        &[
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--quiet",
            "--detach",
            tip,
        ],
        workspace,
        "could not check out the base branch's tip",
    )?;
    // Exit 1 is a merge with conflicts, which is what the agent is for; any other failure
    // is a merge that could not be made.
    let code = git_exit(
        &with_identity(&["merge", "--no-commit", "--no-ff", "--quiet", &work]),
        workspace,
    )?;
    if code > 1 {
        return Err(format!(
            "git could not merge the change with what the base branch gained (exit code {code})"
        ));
    }
    let conflicted: Vec<String> =
        git_output(&["diff", "--name-only", "--diff-filter=U"], workspace)?
            .lines()
            .map(str::to_owned)
            .collect();
    let range = format!("{since}..{tip}");
    let commits: Vec<String> =
        git_output(&["log", "--format=%h %s", "-n", "30", &range], workspace)?
            .lines()
            .map(str::to_owned)
            .collect();
    Ok(Carried::Merged(Merge {
        tip: tip.to_owned(),
        commits,
        overlap,
        conflicted,
    }))
}

/// What the agent left of the merge: no file may still hold a conflict marker. Then the
/// merge is given up as a merge and the resolved tree is kept as uncommitted work on the
/// base's tip, as the mechanical carry leaves it, so that what delivery commits is one
/// commit on current ground. Returns the commit the work stands on.
fn settle_merge(workspace: &Path, merge: &Merge) -> Result<String, String> {
    let mut files: Vec<&String> = merge.overlap.iter().chain(&merge.conflicted).collect();
    files.sort();
    files.dedup();
    let left: Vec<&str> = files
        .into_iter()
        .filter(|file| holds_a_marker(&workspace.join(file)))
        .map(String::as_str)
        .collect();
    if !left.is_empty() {
        return Err(format!(
            "the agent left a conflict marker in {}, so the change was not delivered",
            left.join(", ")
        ));
    }
    git(&["merge", "--quit"], workspace, "could not leave the merge")?;
    git(
        &["reset", "--quiet", "--mixed", &merge.tip],
        workspace,
        "could not put the resolved work on the base branch's tip",
    )?;
    Ok(merge.tip.clone())
}

/// Whether a file has a line git would have written to mark a conflict.
fn holds_a_marker(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|text| {
        text.lines().any(|line| {
            ["<<<<<<<", ">>>>>>>"]
                .iter()
                .any(|mark| line == *mark || line.starts_with(&format!("{mark} ")))
        })
    })
}

/// The exit code of a git command, for one whose failure is an answer.
fn git_exit(args: &[&str], directory: &Path) -> Result<i32, String> {
    let mut command = vec!["git".to_owned()];
    command.extend(args.iter().map(|a| (*a).to_owned()));
    match execute(&command, directory, CLONE_TIMEOUT, true, say) {
        Ok((Ended::Exited(code), _)) => Ok(code),
        _ => Err(format!("git {} could not be run", args.join(" "))),
    }
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

/// How many of its last lines a gate that failed is remembered by, and how many bytes of them.
const GATE_TAIL_LINES: usize = 60;
const GATE_TAIL_BYTES: usize = 4_000;

/// Gates that did not pass: what the run says, and each one that did not with what it printed
/// last, so the implementing agent can be told.
struct GatesFailed {
    summary: String,
    checks: Vec<contracts::v1alpha1::FailedCheck>,
}

impl GatesFailed {
    /// What the run says when it ends on them: the summary, and what each printed.
    fn with_output(&self) -> String {
        let mut text = self.summary.clone();
        for check in self.checks.iter().filter(|check| !check.excerpt.is_empty()) {
            let _ = write!(text, "\n{} printed:\n{}", check.name, check.excerpt);
        }
        text
    }
}

/// A gate that did not pass, as the implementing agent is told of it.
fn failed_gate(name: &str, result: String, excerpt: String) -> contracts::v1alpha1::FailedCheck {
    contracts::v1alpha1::FailedCheck {
        name: format!("gate {name}"),
        result,
        excerpt,
    }
}

/// The last of what a gate printed, cut to what is worth keeping.
fn gate_tail(tail: &heph::process::Tail) -> String {
    let text = tail.text();
    if text.len() <= GATE_TAIL_BYTES {
        return text;
    }
    let mut start = text.len() - GATE_TAIL_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

/// Run one gate. One the workspace could not run, or that timed out, that the repository says a
/// check on the pull request runs too, is left to that check.
#[expect(
    clippy::too_many_arguments,
    reason = "each says one separate thing: which gate, where, what changed, what it may be given, where its output goes, and what to do while it runs"
)]
fn run_one_gate(
    manifest: &Manifest,
    name: &str,
    declared: &heph::manifest::Gate,
    workspace: &Path,
    changed: &[String],
    secrets: &Secrets,
    printed: &heph::process::Tail,
    tick: heph::process::Tick<'_>,
) -> Result<gate::GateResult, gate::NotRun> {
    // A gate's secret is the one the executor holds, or else the environment's.
    let lookup = |variable: &str| {
        secrets
            .gate(variable)
            .map(str::to_owned)
            .or_else(|| environment(variable))
    };
    let command = gate::command_for(manifest, declared, lookup);
    match gate::run_named_with_tail(
        manifest,
        name,
        Some(changed),
        workspace,
        lookup,
        say,
        Some(printed),
        Some(tick),
    ) {
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
    }
}

/// Run every gate the manifest declares in the workspace, in name order, and
/// say what each did.
///
/// Every gate runs even if an earlier one failed, so a failure shows all that is
/// wrong at once. The outer error is a failure to write an event. The inner
/// result is the workload's: why it failed, if any gate did, and what each that
/// failed printed last. A heartbeat is written every `heartbeat` while a gate runs, so a gate
/// that takes minutes is not taken for a run that went silent.
fn run_gates<W: Write>(
    manifest: &Manifest,
    workspace: &Path,
    since: &str,
    secrets: &Secrets,
    heartbeat: Duration,
    stream: &mut Stream<'_, W>,
) -> Result<Result<Vec<deliver::Deferred>, GatesFailed>, Refused> {
    let changed = match changed_files(workspace, since) {
        Ok(changed) => changed,
        Err(reason) => {
            return Ok(Err(GatesFailed {
                summary: reason,
                checks: Vec::new(),
            }));
        }
    };

    let mut failures = Vec::new();
    let mut checks: Vec<contracts::v1alpha1::FailedCheck> = Vec::new();
    let mut deferred = Vec::new();
    for (name, declared) in &manifest.gates {
        if heph::process::cancelled() {
            break;
        }
        stream.emit(Kind::GateStarted(GateStarted { name: name.clone() }))?;
        // What the gate prints is shown as it was, and the last of it is kept in case the gate
        // fails.
        let printed = heph::process::Tail::new(GATE_TAIL_LINES);
        let mut write_failed: Option<std::io::Error> = None;
        let mut beat = || match stream.emit(Kind::Heartbeat(Heartbeat {})) {
            Ok(()) => heph::process::Go::On,
            Err(error) => {
                write_failed = Some(error);
                heph::process::Go::Stop
            }
        };
        let outcome = run_one_gate(
            manifest,
            name,
            declared,
            workspace,
            &changed,
            secrets,
            &printed,
            heph::process::Tick {
                every: heartbeat,
                call: &mut beat,
            },
        );
        if let Some(error) = write_failed {
            return Err(error.into());
        }
        match outcome {
            Ok(result) => {
                if matches!(
                    result.outcome,
                    gate::Outcome::Failed | gate::Outcome::TimedOut
                ) {
                    failures.push(format!("gate {name} {}", result.outcome));
                    checks.push(failed_gate(
                        name,
                        result.outcome.to_string(),
                        gate_tail(&printed),
                    ));
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
            Err(error) => {
                failures.push(format!("gate {name} could not be run: {error}"));
                checks.push(failed_gate(
                    name,
                    "could not be run".to_owned(),
                    error.to_string(),
                ));
            }
        }
    }
    if heph::process::cancelled() {
        failures.push("cancelled while the gates ran".to_owned());
    }
    Ok(if failures.is_empty() {
        Ok(deferred)
    } else {
        Err(GatesFailed {
            summary: failures.join("; "),
            checks,
        })
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
    text.push_str(deliver::QUESTION_FILE);
    text.push('\n');
    text.push_str(deliver::DESCRIPTION_FILE);
    text.push('\n');
    text.push_str(review::VERDICT_FILE);
    text.push('\n');
    text.push_str(completeness::VERDICT_FILE);
    text.push('\n');
    text.push_str(gap::STEP_FILE);
    text.push('\n');
    fs::write(&exclude, text)
        .map_err(|e| format!("could not keep the commit message out of the changes: {e}"))
}

/// Where what a command says about itself goes: standard error, because
/// standard output carries only events.
///
/// What it says has the value of every secret taken out, once [`keep_out_of_the_log`] has
/// said which they are.
pub fn say(message: &str) {
    eprintln!("{}", for_the_log(message));
}

static LOGGED: std::sync::OnceLock<Redactor> = std::sync::OnceLock::new();

/// Take the value of every secret `secrets` holds out of everything [`say`] writes from now on.
/// Called once, before anything is run; a second call changes nothing.
pub fn keep_out_of_the_log(secrets: &Secrets) {
    let _ = LOGGED.set(secrets.redactor());
}

/// `message` as the executor's own log says it.
#[must_use]
pub fn for_the_log(message: &str) -> String {
    LOGGED
        .get()
        .map_or_else(|| message.to_owned(), |redactor| redactor.text(message))
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

/// Write the record, with the value of every secret held taken out of it.
fn write_record(path: &Path, record: &RunRecord, redactor: &Redactor) -> Result<(), Refused> {
    let mut text = serde_json::to_string_pretty(record).map_err(|e| Refused(e.to_string()))?;
    if !redactor.is_empty() {
        let mut value = serde_json::to_value(record).map_err(|e| Refused(e.to_string()))?;
        // A record that holds no secret is written as it was made.
        if redactor.json(&mut value) {
            text = serde_json::to_string_pretty(&value).map_err(|e| Refused(e.to_string()))?;
        }
    }
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
