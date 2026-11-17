//! Checking a repository against the baseline.
//!
//! The items are the ones in the Athenaeum release this crate pins, so the
//! tool is graded against the list it checks. This version answers the items
//! that a working tree can: files, and rules about workflows. An item that
//! needs GitHub is reported as not checked, and never counts as a pass or as a
//! failure, so a repository is not told it is fine on a question nobody asked.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use jsonschema::Validator;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yaml_rust2::{Yaml, YamlLoader};

use crate::gate;
use crate::schema::{self, Invalid};

/// Where a repository keeps its answers, relative to its root.
pub const DEFAULT_ANSWERS: &str = ".athenaeum/baseline.toml";

/// Directories that are never searched: they hold what a build or a package
/// manager put there, and never what the repository itself has.
const SKIPPED_DIRECTORIES: [&str; 3] = [".git", "target", "node_modules"];

/// How many offending places one item's detail names before it counts the rest.
const DETAIL_LIMIT: usize = 10;

/// The baseline, as the pinned release holds it.
#[derive(Debug, Deserialize)]
struct Baseline {
    items: BTreeMap<String, Item>,
}

#[derive(Debug, Deserialize)]
struct Item {
    title: String,
    kind: Kind,
    severity: Severity,
    check: Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    File,
    Workflow,
    Setting,
    Ruleset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Required,
    Recommended,
}

/// The keys a check may have, whichever kind it is. The schema has already
/// said which keys each kind allows.
#[derive(Debug, Deserialize)]
struct Check {
    path_any: Option<Vec<String>>,
    rule: Option<String>,
}

/// A repository's answers, read for what this command needs: its exceptions.
#[derive(Debug, Default, Deserialize)]
struct Answers {
    #[serde(default)]
    exceptions: BTreeMap<String, Exception>,
}

#[derive(Debug, Deserialize)]
struct Exception {
    reason: String,
}

/// What a check found for one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Satisfied,
    Unsatisfied,
    Excepted,
    NotApplicable,
    NotChecked,
}

#[derive(Debug, Serialize)]
struct ItemReport {
    title: String,
    kind: Kind,
    severity: Severity,
    outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// What a check of a repository found.
#[derive(Debug)]
pub struct Checked {
    /// Whether a required item is unsatisfied and not excepted.
    pub failed: bool,
    /// The report a gate result carries.
    pub report: Value,
    /// One line for each item, and a summary, for a person to read.
    pub table: String,
}

/// Why the check could not be made, as distinct from a repository that does
/// not satisfy the baseline.
#[derive(Debug)]
pub enum CouldNotCheck {
    /// The answers could not be read.
    Answers(String),
    /// The answers name an item the baseline does not have.
    UnknownException(Vec<String>),
    /// A file or directory in the repository could not be read.
    Unreadable(String),
    /// A workflow is not YAML.
    Workflow(String),
}

impl std::fmt::Display for CouldNotCheck {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Answers(message) | Self::Unreadable(message) | Self::Workflow(message) => {
                formatter.write_str(message)
            }
            Self::UnknownException(ids) => write!(
                formatter,
                "the answers except an item the baseline does not have: {}",
                ids.join(", ")
            ),
        }
    }
}

static ANSWERS_VALIDATOR: LazyLock<Validator> =
    LazyLock::new(|| schema::compile(athenaeum::baseline_answers::SCHEMA));

/// Check the repository at `root` against the baseline, with the answers in
/// `answers` if the file exists.
///
/// # Errors
///
/// Returns why the check could not be made: the answers are not valid or name
/// an item that does not exist, the tree cannot be read, or a workflow is not
/// YAML.
///
/// # Panics
///
/// Only if the pinned baseline cannot be read, which is a defect in this crate
/// and is ruled out by a test that reads it.
pub fn check(root: &Path, answers: &Path) -> Result<Checked, CouldNotCheck> {
    let baseline: Baseline = toml::from_str(athenaeum::baseline::ITEMS)
        .expect("the pinned baseline to be readable as a Baseline");

    let (answers, present) = read_answers(answers)?;

    let unknown: Vec<String> = answers
        .exceptions
        .keys()
        .filter(|id| !baseline.items.contains_key(*id))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(CouldNotCheck::UnknownException(unknown));
    }

    let files = files_in(root)?;
    let workflows = read_workflows(root, &files)?;

    let mut items = BTreeMap::new();
    let mut failed = false;
    for (id, item) in &baseline.items {
        let (mut outcome, mut detail) = evaluate(item, &files, &workflows);
        let mut reason = None;
        if let Some(exception) = answers.exceptions.get(id) {
            if outcome == Outcome::Unsatisfied {
                outcome = Outcome::Excepted;
                reason = Some(exception.reason.clone());
            } else {
                detail = Some(detail.map_or_else(
                    || "excepted, though it is satisfied".to_owned(),
                    |found| format!("{found}; excepted, though it is not unsatisfied"),
                ));
            }
        }
        if item.severity == Severity::Required && outcome == Outcome::Unsatisfied {
            failed = true;
        }
        items.insert(
            id.clone(),
            ItemReport {
                title: item.title.clone(),
                kind: item.kind,
                severity: item.severity,
                outcome,
                detail,
                reason,
            },
        );
    }

    Ok(Checked {
        failed,
        table: table_of(&items),
        report: json!({
            "answers": if present { "present" } else { "absent" },
            "summary": summary_of(&items),
            "items": items,
        }),
    })
}

/// Read the answers if the file exists. A missing file is not an error: the
/// repository simply has no exceptions.
fn read_answers(path: &Path) -> Result<(Answers, bool), CouldNotCheck> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Answers::default(), false));
        }
        Err(error) => {
            return Err(CouldNotCheck::Answers(format!(
                "{}: cannot be read: {error}",
                path.display()
            )));
        }
    };
    match schema::validate(&ANSWERS_VALIDATOR, &text) {
        Ok(()) => {}
        Err(Invalid::NotToml(reason)) => {
            return Err(CouldNotCheck::Answers(format!(
                "{}: is not TOML: {reason}",
                path.display()
            )));
        }
        Err(Invalid::Violations(violations)) => {
            let found: Vec<String> = violations
                .iter()
                .map(|violation| format!("{}: {}", violation.path, violation.message))
                .collect();
            return Err(CouldNotCheck::Answers(format!(
                "{}: is not valid baseline answers: {}",
                path.display(),
                found.join("; ")
            )));
        }
    }
    let answers =
        toml::from_str(&text).expect("answers the schema accepts to be readable as Answers");
    Ok((answers, true))
}

/// Every file under `root`, as a path relative to it with forward slashes, in
/// order. Directories in [`SKIPPED_DIRECTORIES`] are not entered and a symbolic
/// link is not followed, so the walk stays inside the repository.
fn files_in(root: &Path) -> Result<Vec<String>, CouldNotCheck> {
    let mut found = Vec::new();
    let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| {
            CouldNotCheck::Unreadable(format!("{}: cannot be read: {error}", directory.display()))
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                CouldNotCheck::Unreadable(format!(
                    "{}: cannot be read: {error}",
                    directory.display()
                ))
            })?;
            let kind = entry.file_type().map_err(|error| {
                CouldNotCheck::Unreadable(format!(
                    "{}: cannot be read: {error}",
                    entry.path().display()
                ))
            })?;
            let path = entry.path();
            if kind.is_dir() {
                let name = entry.file_name();
                if !SKIPPED_DIRECTORIES.iter().any(|skipped| name == *skipped) {
                    pending.push(path);
                }
            } else if let Ok(relative) = path.strip_prefix(root) {
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Each workflow file under `.github/workflows`, parsed.
fn read_workflows(root: &Path, files: &[String]) -> Result<Vec<(String, Yaml)>, CouldNotCheck> {
    let mut workflows = Vec::new();
    for file in files {
        let Some(name) = file.strip_prefix(".github/workflows/") else {
            continue;
        };
        let is_yaml = Path::new(name)
            .extension()
            .is_some_and(|extension| extension == "yml" || extension == "yaml");
        if name.contains('/') || !is_yaml {
            continue;
        }
        let text = std::fs::read_to_string(root.join(file))
            .map_err(|error| CouldNotCheck::Workflow(format!("{file}: cannot be read: {error}")))?;
        let mut documents = YamlLoader::load_from_str(&text)
            .map_err(|error| CouldNotCheck::Workflow(format!("{file}: is not YAML: {error}")))?;
        if documents.is_empty() {
            return Err(CouldNotCheck::Workflow(format!("{file}: is empty")));
        }
        workflows.push((file.clone(), documents.swap_remove(0)));
    }
    Ok(workflows)
}

fn evaluate(
    item: &Item,
    files: &[String],
    workflows: &[(String, Yaml)],
) -> (Outcome, Option<String>) {
    match item.kind {
        Kind::File => {
            let paths = item.check.path_any.as_deref().unwrap_or_default();
            match gate::matches_any(paths, files) {
                Ok(true) => (Outcome::Satisfied, None),
                Ok(false) => (
                    Outcome::Unsatisfied,
                    Some(format!("none of these exists: {}", paths.join(", "))),
                ),
                Err(error) => (Outcome::Unsatisfied, Some(error.to_string())),
            }
        }
        Kind::Workflow => {
            if workflows.is_empty() {
                return (
                    Outcome::NotApplicable,
                    Some("the repository has no workflows".to_owned()),
                );
            }
            let violations = match item.check.rule.as_deref() {
                Some("actions-pinned") => unpinned_actions(workflows),
                Some("permissions-declared") => undeclared_permissions(workflows),
                Some("job-timeouts") => jobs_without_timeouts(workflows),
                other => {
                    return (
                        Outcome::NotChecked,
                        Some(format!("this tool does not know the rule {other:?}")),
                    );
                }
            };
            if violations.is_empty() {
                (Outcome::Satisfied, None)
            } else {
                (Outcome::Unsatisfied, Some(describe(&violations)))
            }
        }
        Kind::Setting | Kind::Ruleset => (
            Outcome::NotChecked,
            Some("needs GitHub, which this check does not read".to_owned()),
        ),
    }
}

fn describe(violations: &[String]) -> String {
    let shown: Vec<&str> = violations
        .iter()
        .take(DETAIL_LIMIT)
        .map(String::as_str)
        .collect();
    let rest = violations.len().saturating_sub(DETAIL_LIMIT);
    if rest == 0 {
        shown.join("; ")
    } else {
        format!("{}; and {rest} more", shown.join("; "))
    }
}

/// The jobs of a workflow, by name.
fn jobs_of(workflow: &Yaml) -> Vec<(String, &Yaml)> {
    workflow["jobs"]
        .as_hash()
        .map(|jobs| {
            jobs.iter()
                .filter_map(|(name, job)| name.as_str().map(|name| (name.to_owned(), job)))
                .collect()
        })
        .unwrap_or_default()
}

fn unpinned_actions(workflows: &[(String, Yaml)]) -> Vec<String> {
    let mut found = Vec::new();
    for (file, workflow) in workflows {
        for (job, body) in jobs_of(workflow) {
            let mut uses: Vec<&str> = body["uses"].as_str().into_iter().collect();
            if let Some(steps) = body["steps"].as_vec() {
                uses.extend(steps.iter().filter_map(|step| step["uses"].as_str()));
            }
            for action in uses {
                if !is_pinned(action) {
                    found.push(format!("{file}: job {job} uses {action}"));
                }
            }
        }
    }
    found
}

fn undeclared_permissions(workflows: &[(String, Yaml)]) -> Vec<String> {
    let mut found = Vec::new();
    for (file, workflow) in workflows {
        if !workflow["permissions"].is_badvalue() {
            continue;
        }
        let jobs = jobs_of(workflow);
        if jobs.is_empty() {
            found.push(format!("{file}: declares no permissions"));
        }
        for (job, body) in jobs {
            if body["permissions"].is_badvalue() {
                found.push(format!("{file}: job {job} declares no permissions"));
            }
        }
    }
    found
}

fn jobs_without_timeouts(workflows: &[(String, Yaml)]) -> Vec<String> {
    let mut found = Vec::new();
    for (file, workflow) in workflows {
        for (job, body) in jobs_of(workflow) {
            let calls_a_workflow = !body["uses"].is_badvalue();
            if !calls_a_workflow && body["timeout-minutes"].is_badvalue() {
                found.push(format!("{file}: job {job} has no timeout"));
            }
        }
    }
    found
}

/// Whether an action is used at something that does not move: a full commit, a
/// version tag, a path in the repository, or a docker image with a digest.
/// A branch, or no reference at all, is not.
fn is_pinned(action: &str) -> bool {
    if action.starts_with("./") {
        return true;
    }
    if let Some(image) = action.strip_prefix("docker://") {
        return image.contains("@sha256:");
    }
    let Some((_, reference)) = action.rsplit_once('@') else {
        return false;
    };
    is_commit(reference) || is_version(reference)
}

fn is_commit(reference: &str) -> bool {
    reference.len() == 40
        && reference
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `v4`, `1.99.1`, `v2.3`, `v1.0.0-rc.1`: up to three numbers, then an
/// optional suffix.
fn is_version(reference: &str) -> bool {
    let reference = reference.strip_prefix('v').unwrap_or(reference);
    let (numbers, suffix) = match reference.find(['-', '+']) {
        Some(index) => (&reference[..index], Some(&reference[index + 1..])),
        None => (reference, None),
    };
    let parts: Vec<&str> = numbers.split('.').collect();
    let numeric = (1..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    let suffix_ok = suffix.is_none_or(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
    });
    numeric && suffix_ok
}

fn summary_of(items: &BTreeMap<String, ItemReport>) -> Value {
    let count = |wanted: Outcome| items.values().filter(|item| item.outcome == wanted).count();
    json!({
        "satisfied": count(Outcome::Satisfied),
        "unsatisfied": count(Outcome::Unsatisfied),
        "excepted": count(Outcome::Excepted),
        "not_applicable": count(Outcome::NotApplicable),
        "not_checked": count(Outcome::NotChecked),
    })
}

fn table_of(items: &BTreeMap<String, ItemReport>) -> String {
    let word = |value: Value| value.as_str().map(str::to_owned).unwrap_or_default();
    let mut table = String::new();
    for (id, item) in items {
        let outcome = word(json!(item.outcome));
        let severity = word(json!(item.severity));
        let _ = writeln!(table, "{outcome:<14} {severity:<12} {id}");
        if matches!(item.outcome, Outcome::Unsatisfied | Outcome::Excepted) {
            if let Some(detail) = &item.detail {
                let _ = writeln!(table, "{:<27}{detail}", "");
            }
            if let Some(reason) = &item.reason {
                let _ = writeln!(table, "{:<27}reason: {reason}", "");
            }
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_baseline_is_readable() {
        let baseline: Baseline =
            toml::from_str(athenaeum::baseline::ITEMS).expect("the pinned baseline to be readable");
        assert!(!baseline.items.is_empty());
    }

    #[test]
    fn a_reference_is_pinned_when_it_cannot_move() {
        for pinned in [
            "actions/checkout@v4",
            "owner/tool@1.99.1",
            "owner/tool@v2.3",
            "owner/tool@v1.0.0-rc.1",
            "owner/tool@0123456789abcdef0123456789abcdef01234567",
            "./.github/actions/local",
            "docker://alpine@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ] {
            assert!(is_pinned(pinned), "{pinned} should be pinned");
        }
        for moving in [
            "owner/tool@main",
            "owner/tool@master",
            "owner/tool@stable",
            "owner/tool@release-1",
            "owner/tool",
            "owner/tool@",
            "owner/tool@v",
            "owner/tool@1.2.3.4",
            "owner/tool@0123456789abcdef0123456789abcdef0123456",
            "docker://alpine:3.20",
        ] {
            assert!(!is_pinned(moving), "{moving} should not be pinned");
        }
    }
}
