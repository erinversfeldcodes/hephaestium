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
