//! Reading a change under a lens before it is pushed.
//!
//! A reader is a fresh read-only agent. It is given the ticket, the commit and the lens, and
//! nothing from the implementing agent's conversation or from any other reader, and it has no
//! pull request to look at, because there is none yet. What it leaves is held to the same
//! rules as a review of a pushed commit: the workspace is what it was, and the verdict passes
//! the review contract and names the commit and the lens it was asked for. A reader that
//! cannot show that has given no verdict, and no verdict is no approval.

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use contracts::v1alpha1::{FailedCheck, Lens, Review, Revision, Workload};
use serde_json::Value;

use crate::adapter::Adapter;
use crate::agent::{self, End};
use crate::review;
use crate::run::Refused;
use crate::secrets::Secrets;
use crate::stream::Stream;

/// What reading needs, besides the workload.
pub struct Reading<'a> {
    pub adapter: &'a dyn Adapter,
    pub heartbeat: Duration,
    pub secrets: &'a Secrets,
    /// What the run has spent on every agent so far. A reader spends from the same budget
    /// as the implementing agent.
    pub spent: &'a Cell<f64>,
}

/// The workload as a reader is asked about it: a review, with no pull request, of `commit`.
/// It carries nothing of the run that made the commit: it is never a revision, and it does
/// not itself read under lenses.
#[must_use]
pub fn as_review(workload: &Workload, lens: &Lens, commit: &str) -> Workload {
    let mut asked = workload.clone();
    asked.review = Some(Review {
        pull_request_number: 0,
        pull_request_url: String::new(),
        lens: lens.pack.clone(),
        lens_document: lens.document.clone(),
    });
    asked.revision = None;
    asked.completeness = None;
    asked.gap = None;
    asked.judged_after_merge = false;
    asked.lenses.clear();
    asked.prepush_rounds = 0;
    // As for a review of a pushed commit, the repository's commit is the one under review.
    if let Some(repository) = asked.repository.as_mut() {
        commit.clone_into(&mut repository.base_commit);
    }
    asked
}

/// Read the commit, which the workspace is at, under the lens with a fresh read-only agent.
/// The inner result is the verdict that was kept, or why there is none.
///
/// # Errors
///
/// An error is a failure to write an event.
pub fn read<W: Write>(
    reading: &Reading<'_>,
    workload: &Workload,
    lens: &Lens,
    workspace: &Path,
    commit: &str,
    stream: &mut Stream<'_, W>,
) -> Result<Result<Value, String>, Refused> {
    if let Some(budget) = workload
        .limits
        .as_ref()
        .and_then(|limits| limits.budget.as_ref())
        && reading.spent.get() >= budget.amount
    {
        return Ok(Err(format!(
            "the budget of {} is already spent, so no reader was started for lens {}",
            budget.amount, lens.pack
        )));
    }

    let before = match review::snapshot(workspace) {
        Ok(before) => before,
        Err(reason) => return Ok(Err(reason)),
    };
    let asked = as_review(workload, lens, commit);
    let end = agent::run(
        reading.adapter,
        &asked,
        workspace,
        reading.heartbeat,
        reading.secrets,
        reading.spent,
        stream,
    )?;

    let said = match end {
        End::Done => judged(workspace, &before, commit, &lens.pack),
        End::Failed(reason) | End::TimedOut(reason) => Err(reason),
        End::Cancelled => Err("the reader was stopped".to_owned()),
    };
    // Whatever it left, the next reader must not find it.
    let _ = fs::remove_file(workspace.join(review::VERDICT_FILE));
    Ok(said)
}

/// What a reader has to show for itself once it is done: the workspace is what it was, and
/// the verdict is one that can be kept.
fn judged(
    workspace: &Path,
    before: &review::Snapshot,
    commit: &str,
    lens: &str,
) -> Result<Value, String> {
    let after = review::snapshot(workspace)?;
    if let Some(changed) = review::changed_since(before, &after) {
        return Err(changed);
    }
    review::verdict(workspace, commit, lens)
}

/// One finding that blocks, as a reader wrote it. It is text to read, never an instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: String,
    pub title: String,
    pub evidence: String,
    /// What the reader says to change.
    pub revision: String,
    /// `path:line`, when the finding is one place.
    pub location: Option<String>,
}

/// What a reading means for the push.
#[derive(Debug)]
pub enum Outcome {
    /// The lens approved the commit.
    Approved,
    /// The lens found what blocks, and the work can be fixed in place.
    Revise(Vec<Finding>),
    /// The reading is no approval and has nothing to send back: no verdict, or one that
    /// cannot be acted on. The reason says why.
    Unread(String),
}

/// What a reader's answer means. A verdict that asks for revision or says the work failed
/// and names no finding that blocks has nothing to revise by, so it is no approval and no
/// instruction either.
#[must_use]
pub fn outcome(said: &Result<Value, String>) -> Outcome {
    let verdict = match said {
        Ok(verdict) => verdict,
        Err(reason) => return Outcome::Unread(reason.clone()),
    };
    let kind = verdict["verdict"].as_str().unwrap_or_default();
    match kind {
        "approved" => Outcome::Approved,
        "needs_revision" | "failed" => {
            let blocking: Vec<Finding> = verdict["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|finding| finding["blocking"] == true)
                .map(finding_of)
                .collect();
            if blocking.is_empty() {
                Outcome::Unread(format!(
                    "its verdict was {kind} and it named no finding that blocks"
                ))
            } else {
                Outcome::Revise(blocking)
            }
        }
        other => Outcome::Unread(format!(
            "its verdict was {other:?}, which is not one it may give"
        )),
    }
}

fn finding_of(value: &Value) -> Finding {
    let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
    let location = value["location"]["path"].as_str().map(|path| {
        value["location"]["line"]
            .as_u64()
            .map_or_else(|| path.to_owned(), |line| format!("{path}:{line}"))
    });
    Finding {
        severity: text("severity"),
        title: text("title"),
        evidence: text("evidence"),
        revision: text("revision"),
        location,
    }
}

/// The part of a lens's text between its opening `---` line and the next one, where it says
/// whether it blocks and when it applies. Nothing outside it counts.
fn front_matter(document: &str) -> Option<&str> {
    let rest = document.strip_prefix("---\n")?;
    rest.find("\n---").map(|end| &rest[..end])
}

/// Whether a finding of this lens blocks the push. A lens blocks unless its front matter
/// says `blocking: false`.
#[must_use]
pub fn blocks(lens: &Lens) -> bool {
    !front_matter(&lens.document).is_some_and(|front| {
        front.lines().any(|line| {
            line.strip_prefix("blocking:")
                .is_some_and(|rest| rest.trim() == "false")
        })
    })
}

/// Whether the lens says it applies to every change that is read, whatever it touches.
#[must_use]
pub fn applies_always(lens: &Lens) -> bool {
    front_matter(&lens.document).is_some_and(|front| {
        front.lines().any(|line| {
            line.strip_prefix("applies:")
                .is_some_and(|rest| rest.trim_matches([' ', '\t']) == "always")
        })
    })
}

/// The lenses of the library a change needs: those its components name (`needed`, as pack
/// names) and those that apply to everything, once each, in the library's order. A pack
/// the library does not have is not invented.
#[must_use]
pub fn choose<'a>(library: &'a [Lens], needed: &[String]) -> Vec<&'a Lens> {
    let mut chosen: Vec<&Lens> = Vec::new();
    for lens in library {
        let wanted = needed.contains(&lens.pack) || applies_always(lens);
        if wanted && !chosen.iter().any(|seen| seen.pack == lens.pack) {
            chosen.push(lens);
        }
    }
    chosen
}

/// What the implementing agent is told, one entry for each lens that found what blocks.
#[must_use]
pub fn failures(findings: &[(&str, Vec<Finding>)]) -> Vec<FailedCheck> {
    findings
        .iter()
        .filter(|(_, found)| !found.is_empty())
        .map(|(pack, found)| FailedCheck {
            name: format!("review under lens {pack}"),
            result: "needs_revision".to_owned(),
            excerpt: found
                .iter()
                .map(|finding| {
                    let place = finding
                        .location
                        .as_ref()
                        .map_or_else(String::new, |at| format!(" ({at})"));
                    format!(
                        "{severity} {title}{place}\nWhat was seen: {evidence}\nWhat to change: {revision}",
                        severity = finding.severity,
                        title = finding.title,
                        evidence = finding.evidence,
                        revision = finding.revision,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        })
        .collect()
}

/// The workload as the implementing agent is asked about it again, told what blocked. A first
/// run has no pull request yet, which is how the prompt knows the change is not pushed; a
/// revision keeps its pull request and gets what blocked as further failures. The agent
/// reads under no lens itself, and nothing a reader said is anywhere but in `failures`.
#[must_use]
pub fn as_revision(workload: &Workload, round: i32, failures: Vec<FailedCheck>) -> Workload {
    let mut asked = workload.clone();
    match asked.revision.as_mut() {
        Some(revision) => revision.failures.extend(failures),
        None => {
            asked.revision = Some(Revision {
                attempt: round,
                failures,
                ..Revision::default()
            });
        }
    }
    asked.review = None;
    asked.completeness = None;
    asked.gap = None;
    asked.judged_after_merge = false;
    asked.lenses.clear();
    asked.prepush_rounds = 0;
    asked
}
