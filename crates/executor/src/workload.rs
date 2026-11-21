//! Reading a workload, and refusing one that is not safe to use.

use std::path::{Component, Path};

use contracts::v1alpha1::Workload;

/// Where a manifest is when the workload does not say.
pub const DEFAULT_MANIFEST: &str = ".athenaeum/manifest.toml";

/// Read a workload strictly: a field the contract does not have, a name it
/// does not know or a value of the wrong type is an error.
///
/// # Errors
///
/// Returns why the text is not a workload, or why a value in it is not safe.
pub fn read(text: &str) -> Result<Workload, String> {
    let workload: Workload = serde_json::from_str(text).map_err(|error| error.to_string())?;
    check_values(&workload)?;
    Ok(workload)
}

/// The values the executor uses directly. The commit goes to git, the id names a
/// directory and the manifest path is opened under the workspace, so each must
/// be what it is meant to be and nothing that could be taken for an option or a
/// way out of the workspace.
fn check_values(workload: &Workload) -> Result<(), String> {
    let repository = workload
        .repository
        .as_ref()
        .ok_or("the workload names no repository")?;

    if workload.id.is_empty()
        || !workload
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "the workload id {:?} is not made only of letters, digits, - and _",
            workload.id
        ));
    }

    let commit = &repository.base_commit;
    if commit.len() != 40 || !commit.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return Err(format!(
            "the base commit {commit:?} is not forty lower-case hexadecimal digits"
        ));
    }

    check_limits(workload)?;
    check_revision(workload)?;
    check_completeness(workload)?;
    check_gap(workload)?;
    check_review(workload)?;

    if !repository.manifest_path.is_empty()
        && !Path::new(&repository.manifest_path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "the manifest path {:?} is not a path inside the repository",
            repository.manifest_path
        ));
    }
    Ok(())
}

/// What a revision is pushed to and told. The branch goes to `git push`, so it
/// must be one the runtime made and nothing that could be taken for an option or
/// a way to another ref; the pull request must be a GitHub address, and there
/// must be something that failed.
fn check_revision(workload: &Workload) -> Result<(), String> {
    let Some(revision) = workload.revision.as_ref() else {
        return Ok(());
    };
    let branch = &revision.branch;
    let rest = branch.strip_prefix("heph/");
    if rest.is_none_or(|rest| {
        rest.is_empty()
            || rest
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || !rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
    }) {
        return Err(format!(
            "the revision's branch {branch:?} is not one the runtime made (heph/ and then letters, digits, - _ . and /)"
        ));
    }
    if revision.pull_request_number <= 0 {
        return Err("the revision's pull request number is not above zero".to_owned());
    }
    if !revision.pull_request_url.starts_with("https://github.com/") {
        return Err(format!(
            "the revision's pull request {:?} is not a GitHub address",
            revision.pull_request_url
        ));
    }
    if revision.attempt <= 0 {
        return Err("the revision's attempt is not above zero".to_owned());
    }
    match revision.base_moved.as_ref() {
        Some(moved) if moved.commits.iter().all(|c| c.trim().is_empty()) => {
            return Err(
                "the revision says the base moved but names no commit that came in".to_owned(),
            );
        }
        None if revision.failures.is_empty() => {
            return Err("the revision names no failing check".to_owned());
        }
        _ => {}
    }
    if revision.failures.iter().any(|f| f.name.trim().is_empty()) {
        return Err("a failing check in the revision has no name".to_owned());
    }
    Ok(())
}

/// What an evaluation is told. It is never also a revision or a review: each ends a
/// different way, and an evaluation changes nothing.
fn check_completeness(workload: &Workload) -> Result<(), String> {
    let Some(completeness) = workload.completeness.as_ref() else {
        return Ok(());
    };
    if workload.revision.is_some() {
        return Err("the workload is both a revision and a completeness evaluation".to_owned());
    }
    if workload.review.is_some() {
        return Err("the workload is both a review and a completeness evaluation".to_owned());
    }
    if completeness.skill_document.trim().is_empty() {
        return Err("the completeness evaluation carries no skill to follow".to_owned());
    }
    Ok(())
}

/// What a step of the questioning is told. It is never also another kind of workload: each
/// ends a different way, and a step changes nothing.
fn check_gap(workload: &Workload) -> Result<(), String> {
    let Some(gap) = workload.gap.as_ref() else {
        return Ok(());
    };
    for (present, kind) in [
        (workload.revision.is_some(), "a revision"),
        (workload.review.is_some(), "a review"),
        (workload.completeness.is_some(), "a completeness evaluation"),
    ] {
        if present {
            return Err(format!("the workload is both {kind} and a gap step"));
        }
    }
    if gap.skill_document.trim().is_empty() {
        return Err("the gap step carries no skill to follow".to_owned());
    }
    Ok(())
}

/// What a review is told. The lens names a pack and is used to name a file, so
/// it is a name and not a path; the pull request must be a GitHub address; and a
/// workload is never a revision and a review at once.
fn check_review(workload: &Workload) -> Result<(), String> {
    let Some(review) = workload.review.as_ref() else {
        return Ok(());
    };
    if workload.revision.is_some() {
        return Err("the workload is both a revision and a review".to_owned());
    }
    if review.pull_request_number <= 0 {
        return Err("the review's pull request number is not above zero".to_owned());
    }
    if !review.pull_request_url.starts_with("https://github.com/") {
        return Err(format!(
            "the review's pull request {:?} is not a GitHub address",
            review.pull_request_url
        ));
    }
    if !review.lens.starts_with(|c: char| c.is_ascii_alphanumeric())
        || !review
            .lens
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(format!(
            "the review's lens {:?} is not a pack name (letters, digits, - and _, and not starting with - or _)",
            review.lens
        ));
    }
    if review.lens_document.trim().is_empty() {
        return Err("the review has no lens text".to_owned());
    }
    Ok(())
}

/// Where the manifest is, relative to the workspace.
#[must_use]
pub fn manifest_path(workload: &Workload) -> &str {
    match workload
        .repository
        .as_ref()
        .map(|r| r.manifest_path.as_str())
    {
        Some(path) if !path.is_empty() => path,
        _ => DEFAULT_MANIFEST,
    }
}

/// A workload never runs without an end: it has a timeout above zero, and a
/// budget in a stated unit above zero.
fn check_limits(workload: &Workload) -> Result<(), String> {
    let limits = workload
        .limits
        .as_ref()
        .ok_or("the workload has no limits")?;
    let timeout = limits
        .timeout
        .as_ref()
        .ok_or("the workload has no timeout")?;
    if timeout.seconds < 0 || (timeout.seconds == 0 && timeout.nanos <= 0) {
        return Err("the workload's timeout is not above zero".to_owned());
    }
    let budget = limits.budget.as_ref().ok_or("the workload has no budget")?;
    if budget.unit == 0 {
        return Err("the workload's budget has no unit".to_owned());
    }
    if !(budget.amount.is_finite() && budget.amount > 0.0) {
        return Err("the workload's budget is not above zero".to_owned());
    }
    Ok(())
}
