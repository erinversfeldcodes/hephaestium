//! Running a review as a review.
//!
//! A review reads one commit under one lens and ends with a verdict. It changes
//! nothing, so the executor holds it to that: the workspace is what it was before
//! the agent ran, and the verdict the agent wrote passes the review contract and
//! names the commit and lens it was asked for. Nothing reads an agent's own claim
//! of approval from what it said.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::Duration;

use contracts::v1alpha1::{Review, Workload};
use heph::process::{Ended, execute};
use serde_json::Value;

use crate::deliver::DESCRIPTION_FILE;
use crate::run::say;

/// Where the agent leaves its verdict, at the workspace root. The executor keeps
/// it out of the workspace's changes.
pub const VERDICT_FILE: &str = ".heph-verdict.json";

const GIT_TIMEOUT: Duration = Duration::from_secs(600);

/// What the agent is told for a review: what the change was for, which commit it
/// is reading and against what, the lens, and where to put the verdict and what
/// that must be. When the implementer left a description, it is given as their
/// account of the change: proof they say they showed lives there and not in the
/// diff, but it is not evidence by itself.
#[must_use]
pub fn prompt(workload: &Workload, review: &Review, description: Option<&str>) -> String {
    let ticket = workload.ticket.clone().unwrap_or_default();
    let repository = workload.repository.clone().unwrap_or_default();
    let lens_fence = fence_for(&review.lens_document);
    let schema_fence = fence_for(athenaeum::review::SCHEMA);
    // A review of a commit that is not pushed yet has no pull request to name.
    let (opening, subject) = if review.pull_request_url.is_empty() {
        (
            format!(
                "You are reviewing a change in a clone of the repository, in the current directory. It was made for ticket #{number}: {title}\n\n{body}",
                number = ticket.number,
                title = ticket.title,
                body = ticket.body,
            ),
            "the change",
        )
    } else {
        (
            format!(
                "You are reviewing a pull request in a clone of the repository, in the current directory. The pull request is {url}. It was opened for ticket #{number}: {title}\n\n{body}",
                url = review.pull_request_url,
                number = ticket.number,
                title = ticket.title,
                body = ticket.body,
            ),
            "the pull request",
        )
    };
    let account = description
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            let fence = fence_for(text);
            format!(
                "The implementer's account of the change follows, as they wrote it in {DESCRIPTION_FILE}. It is their claim and not evidence: it is not part of the diff, and a claim in it that the diff and the files do not support is itself a finding. Use it to find where to look, and to see what they say they showed.\n\n{fence}\n{text}\n{fence}\n\n"
            )
        })
        .unwrap_or_default();
    format!(
        "{opening}\n\n{account}The working tree is at commit {commit}, the commit under review. Read what {subject} changes with `git diff origin/{base}...HEAD`, and read the files around it as you need to.\n\nYou review under one lens, `{lens}`. The lens says what to look for; judge the change by it and by nothing else, and say only what you can point to.\n\n{lens_fence}\n{lens_text}\n{lens_fence}\n\nDo not change any file, do not commit and do not push: a review reads. When you have finished, write your verdict as JSON to `{file}` in the current directory. It must name commit `{commit}` and lens `{lens}`, and must satisfy this JSON Schema, including its rules (an approval holds no blocking finding, and every blocking finding says what to revise):\n\n{schema_fence}\n{schema}\n{schema_fence}\n\nThen say in one short paragraph what you found.",
        commit = repository.base_commit,
        base = repository.base_branch,
        lens = review.lens,
        lens_text = review.lens_document,
        file = VERDICT_FILE,
        schema = athenaeum::review::SCHEMA,
    )
}

/// A fence longer than any run of backticks in the text, so the text cannot end
/// it early.
fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// What the workspace is, to be compared with what it is after the agent ran: the
/// commit it is at, which files git sees as changed or new, and what is changed in
/// the ones it tracks.
pub struct Snapshot {
    head: String,
    status: Vec<String>,
    diff: String,
}

/// Look at the workspace.
///
/// # Errors
///
/// Returns why git could not say.
pub fn snapshot(workspace: &Path) -> Result<Snapshot, String> {
    let head = git_text(workspace, &["rev-parse", "HEAD"])?
        .trim()
        .to_owned();
    let status = git_text(
        workspace,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?
    .split('\0')
    .filter(|entry| !entry.is_empty())
    .map(str::to_owned)
    .collect();
    let diff = git_text(workspace, &["diff", "HEAD", "--binary"])?;
    Ok(Snapshot { head, status, diff })
}

/// Why a review that ran did not change nothing, or `None` when it changed nothing.
#[must_use]
pub fn changed_since(before: &Snapshot, after: &Snapshot) -> Option<String> {
    changed_since_by("review", before, after)
}

/// Why a read-only run, named `what` in the reason, did not change nothing, or `None`
/// when it changed nothing.
#[must_use]
pub fn changed_since_by(what: &str, before: &Snapshot, after: &Snapshot) -> Option<String> {
    if before.head != after.head {
        return Some(format!(
            "the {what} moved HEAD from {} to {}: it made a commit",
            before.head, after.head
        ));
    }
    let files: Vec<&str> = after
        .status
        .iter()
        .filter(|entry| !before.status.contains(entry))
        .map(|entry| entry.get(3..).unwrap_or(entry.as_str()))
        .collect();
    if !files.is_empty() {
        return Some(format!(
            "the {what} changed the workspace: {}",
            files.join(", ")
        ));
    }
    (before.diff != after.diff)
        .then(|| format!("the {what} changed the workspace: a file it was already changed in"))
}

/// The verdict the agent left, if it is a verdict that passes the review contract
/// and names the commit and lens that were asked for.
///
/// # Errors
///
/// Returns why it is not: it is missing, is not JSON, breaks the contract, or
/// names another commit or lens.
pub fn verdict(workspace: &Path, commit: &str, lens: &str) -> Result<Value, String> {
    let file = workspace.join(VERDICT_FILE);
    let text = fs::read_to_string(&file)
        .map_err(|error| format!("the review left no verdict at {VERDICT_FILE} ({error})"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("the verdict is not JSON: {error}"))?;

    let validator = heph::schema::compile(athenaeum::review::SCHEMA);
    let mut broken = String::new();
    for error in validator.iter_errors(&value) {
        let _ = write!(
            broken,
            "{}{}: {error}",
            if broken.is_empty() { "" } else { "; " },
            error.instance_path().as_str()
        );
    }
    if !broken.is_empty() {
        return Err(format!(
            "the verdict does not pass the review contract: {broken}"
        ));
    }

    let named = value["commit"].as_str().unwrap_or_default();
    if named != commit {
        return Err(format!(
            "the verdict is for commit {named}, and the commit under review is {commit}"
        ));
    }
    let named = value["lens"].as_str().unwrap_or_default();
    if named != lens {
        return Err(format!(
            "the verdict is under lens {named}, and the lens asked for is {lens}"
        ));
    }
    Ok(value)
}

fn git_text(workspace: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = vec!["git".to_owned()];
    command.extend(args.iter().map(|a| (*a).to_owned()));
    match execute(&command, workspace, GIT_TIMEOUT, true, say) {
        Ok((Ended::Exited(0), Some(bytes))) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        _ => Err(format!(
            "could not look at the workspace with git {}",
            args.join(" ")
        )),
    }
}
