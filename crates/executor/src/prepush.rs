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

use contracts::v1alpha1::{Lens, Review, Workload};
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
