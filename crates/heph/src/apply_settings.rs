//! Changing a repository's GitHub settings and creating its default-branch
//! ruleset, so that the baseline's `setting` and `ruleset` items are satisfied.
//!
//! This changes things outside the working tree, so it is built to be hard to
//! do by accident. It reads GitHub first and works out every request before
//! making any. It makes none unless it was told to. It never modifies a
//! ruleset that exists: it creates one only where nothing covers the default
//! branch. A request GitHub refuses is reported and does not stop the others.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::baseline::{self, CouldNotCheck};
use crate::github::{self, Rulesets, State};

/// How one change went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Worked out, and not made, because it was not asked for.
    Planned,
    /// Made, and GitHub accepted it.
    Applied,
    /// Made, and GitHub refused it.
    Failed,
}

impl Outcome {
    fn word(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Applied => "applied",
            Self::Failed => "failed",
        }
    }
}

/// One request to GitHub.
#[derive(Debug)]
struct Change {
    items: Vec<String>,
    method: &'static str,
    endpoint: String,
    body: Option<Value>,
    outcome: Outcome,
    message: Option<String>,
}

/// What was done, or would be.
#[derive(Debug)]
pub struct Applied {
    /// Whether a request was refused.
    pub failed: bool,
    /// The report a gate result carries.
    pub report: Value,
    /// One line for each thing, for a person to read.
    pub table: String,
}

/// Why nothing could be applied.
#[derive(Debug)]
pub struct CouldNotApply(pub String);

impl std::fmt::Display for CouldNotApply {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<CouldNotCheck> for CouldNotApply {
    fn from(error: CouldNotCheck) -> Self {
        Self(error.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct Items {
    items: BTreeMap<String, Item>,
}

#[derive(Debug, Deserialize)]
struct Item {
    kind: String,
    check: Check,
}

#[derive(Debug, Deserialize)]
struct Check {
    setting: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Answers {
    required_checks: Vec<String>,
    default_branch_bypass: String,
}

/// Work out what to change for the repository at `root`, and change it if `yes`.
///
/// # Errors
///
/// Returns why nothing could be applied: the answers are missing or not valid,
/// or GitHub could not be read.
///
/// # Panics
///
/// Only if the pinned baseline cannot be read, which is a defect in this crate
/// and is ruled out by a test that reads it.
pub fn apply(
    root: &Path,
    answers_path: &Path,
    repository: &str,
    yes: bool,
) -> Result<Applied, CouldNotApply> {
    let answers_text = std::fs::read_to_string(answers_path).map_err(|error| {
        CouldNotApply(format!(
            "the repository's answers are needed, because the ruleset needs its checks and its bypass, and {}: {error}",
            answers_path.display()
        ))
    })?;
    let state = github::read(repository).map_err(|error| CouldNotApply(error.to_string()))?;
    let checked = baseline::check(root, answers_path, Some(&state))?;
    let answers: Answers = toml::from_str(&answers_text)
        .map_err(|error| CouldNotApply(format!("{}: {error}", answers_path.display())))?;
    let items: Items = toml::from_str(athenaeum::baseline::ITEMS)
        .expect("the pinned baseline to be readable as a list of items");

    let (mut changes, skipped) = plan(&items, &checked.report, &state, &answers, repository);

    if yes {
        for change in &mut changes {
            let made = github::write(change.method, &change.endpoint, change.body.as_ref())
                .map_err(|error| CouldNotApply(error.to_string()))?;
            match made {
                Ok(()) => change.outcome = Outcome::Applied,
                Err(message) => {
                    change.outcome = Outcome::Failed;
                    change.message = Some(message);
                }
            }
        }
    }

    let failed = changes
        .iter()
        .any(|change| change.outcome == Outcome::Failed);
    Ok(Applied {
        failed,
        table: table_of(&changes, &skipped),
        report: json!({
            "dry_run": !yes,
            "changes": changes.iter().map(change_json).collect::<Vec<_>>(),
            "skipped": skipped
                .iter()
                .map(|(item, reason)| json!({"item": item, "reason": reason}))
                .collect::<Vec<_>>(),
        }),
    })
}

/// The requests to make, and the items that will not be applied and why.
fn plan(
    items: &Items,
    report: &Value,
    state: &State,
    answers: &Answers,
    repository: &str,
) -> (Vec<Change>, Vec<(String, String)>) {
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    let mut ruleset_items = Vec::new();
    let base = format!("repos/{repository}");

    for (id, item) in &items.items {
        if report["items"][id]["outcome"] != "unsatisfied" {
            continue;
        }
        match (item.kind.as_str(), item.check.setting.as_deref()) {
            ("setting", Some("secret-scanning")) => changes.push(change(
                id,
                "PATCH",
                &base,
                Some(json!({"security_and_analysis": {"secret_scanning": {"status": "enabled"}}})),
            )),
            ("setting", Some("secret-scanning-push-protection")) => changes.push(change(
                id,
                "PATCH",
                &base,
                Some(json!({"security_and_analysis": {"secret_scanning_push_protection": {"status": "enabled"}}})),
            )),
            ("setting", Some("dependabot-alerts")) => {
                changes.push(change(id, "PUT", &format!("{base}/vulnerability-alerts"), None));
            }
            ("setting", Some("dependabot-security-updates")) => {
                changes.push(change(id, "PUT", &format!("{base}/automated-security-fixes"), None));
            }
            ("setting", Some("delete-branch-on-merge")) => changes.push(change(
                id,
                "PATCH",
                &base,
                Some(json!({"delete_branch_on_merge": true})),
            )),
            ("setting", Some("code-scanning")) => skipped.push((
                id.clone(),
                "needs a choice of languages and a query suite, which this does not make for you".to_owned(),
            )),
            ("setting", Some("description")) => skipped.push((
                id.clone(),
                "needs text only the maintainer can write".to_owned(),
            )),
            ("ruleset", _) => ruleset_items.push(id.clone()),
            _ => {}
        }
    }

    if !ruleset_items.is_empty() {
        plan_ruleset(
            &base,
            state,
            answers,
            ruleset_items,
            &mut changes,
            &mut skipped,
        );
    }
    (changes, skipped)
}

/// Create the ruleset where no ruleset covers the default branch. Where one
/// does, change nothing, and say what it lacks.
fn plan_ruleset(
    base: &str,
    state: &State,
    answers: &Answers,
    ruleset_items: Vec<String>,
    changes: &mut Vec<Change>,
    skipped: &mut Vec<(String, String)>,
) {
    let all = match &state.rulesets {
        Rulesets::Known(all) => all,
        Rulesets::Unknown(why) => {
            for id in ruleset_items {
                skipped.push((id, format!("the rulesets could not be read: {why}")));
            }
            return;
        }
    };
    let default_branch = state.repository["default_branch"]
        .as_str()
        .unwrap_or("main");
    if all
        .iter()
        .any(|ruleset| baseline::covers(ruleset, default_branch))
    {
        for id in ruleset_items {
            skipped.push((
                id,
                "an existing ruleset covers the default branch and lacks this; an existing ruleset is never modified"
                    .to_owned(),
            ));
        }
        return;
    }
    changes.push(Change {
        items: ruleset_items,
        method: "POST",
        endpoint: format!("{base}/rulesets"),
        body: Some(ruleset_body(answers)),
        outcome: Outcome::Planned,
        message: None,
    });
}

/// The ruleset that satisfies every ruleset item for these answers.
fn ruleset_body(answers: &Answers) -> Value {
    let checks: Vec<Value> = answers
        .required_checks
        .iter()
        .map(|context| json!({"context": context}))
        .collect();
    let bypass = if answers.default_branch_bypass == "none" {
        json!([])
    } else {
        json!([{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "pull_request"}])
    };
    json!({
        "name": "default branch",
        "target": "branch",
        "enforcement": "active",
        "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
        "bypass_actors": bypass,
        "rules": [
            {"type": "deletion"},
            {"type": "non_fast_forward"},
            {"type": "required_linear_history"},
            {"type": "pull_request", "parameters": {
                "required_approving_review_count": 1,
                "dismiss_stale_reviews_on_push": true,
                "require_code_owner_review": false,
                "require_last_push_approval": true,
                "required_review_thread_resolution": true,
                "allowed_merge_methods": ["squash", "rebase"]
            }},
            {"type": "required_status_checks", "parameters": {
                "required_status_checks": checks,
                "strict_required_status_checks_policy": true,
                "do_not_enforce_on_create": false
            }}
        ]
    })
}

fn change(item: &str, method: &'static str, endpoint: &str, body: Option<Value>) -> Change {
    Change {
        items: vec![item.to_owned()],
        method,
        endpoint: endpoint.to_owned(),
        body,
        outcome: Outcome::Planned,
        message: None,
    }
}

fn change_json(change: &Change) -> Value {
    let mut entry = json!({
        "items": change.items,
        "method": change.method,
        "endpoint": change.endpoint,
        "outcome": change.outcome.word(),
    });
    if let Some(body) = &change.body {
        entry["body"] = body.clone();
    }
    if let Some(message) = &change.message {
        entry["message"] = json!(message);
    }
    entry
}

fn table_of(changes: &[Change], skipped: &[(String, String)]) -> String {
    use std::fmt::Write as _;
    let mut table = String::new();
    for change in changes {
        let _ = writeln!(
            table,
            "{:<8} {} {}  ({})",
            change.outcome.word(),
            change.method,
            change.endpoint,
            change.items.join(", ")
        );
        if let Some(message) = &change.message {
            let _ = writeln!(table, "{:<8} {message}", "");
        }
    }
    for (item, reason) in skipped {
        let _ = writeln!(table, "{:<8} {item}: {reason}", "skipped");
    }
    table
}
