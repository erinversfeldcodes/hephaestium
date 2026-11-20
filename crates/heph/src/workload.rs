//! Building the workload for a ticket.
//!
//! A workload is what `executor run` is told: the ticket as a maintainer
//! approved it, the repository and the commit to start from, the agent runtime,
//! its limits, and who approved it. Here it is built from what GitHub says, so
//! that a person can run Hephaestium from inside a project with one command,
//! and so that anything else that needs a workload builds it the same way.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use contracts::v1alpha1::{
    AgentRuntime, Approval, Budget, CostUnit, Limits, Repository, Ticket, Workload, agent_runtime,
};
use pbjson_types::{Duration, Timestamp};
use serde_json::Value;

use crate::github::{self, Fetched};

/// The model a workload asks for when nothing says otherwise.
pub const DEFAULT_MODEL: &str = "sonnet";

/// How long a workload may run when nothing says otherwise: thirty minutes.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 1800;

/// What a workload may spend, in dollars, when nothing says otherwise.
pub const DEFAULT_BUDGET_USD: f64 = 5.0;

/// What to build a workload from.
pub struct Request<'a> {
    /// `OWNER/NAME`.
    pub repository: &'a str,
    pub ticket: i64,
    pub model: &'a str,
    pub timeout_seconds: u64,
    pub budget_usd: f64,
    /// Who approved the ticket, when it is not whoever is signed in to `gh`.
    pub approver: Option<&'a str>,
    /// When it was approved, when that is not now.
    pub approved_at: Option<Timestamp>,
}

/// A workload, and what the person who asked for it should be told.
pub struct Built {
    pub workload: Workload,
    pub warnings: Vec<String>,
}

/// Why a workload could not be built.
#[derive(Debug)]
pub struct CouldNotBuild(pub String);

impl std::fmt::Display for CouldNotBuild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<github::CouldNotRead> for CouldNotBuild {
    fn from(error: github::CouldNotRead) -> Self {
        Self(error.to_string())
    }
}

/// Read one endpoint that must exist.
fn found(endpoint: &str, missing: &str) -> Result<Value, CouldNotBuild> {
    match github::get(endpoint)? {
        Fetched::Found(value) => Ok(value),
        Fetched::NotFound => Err(CouldNotBuild(missing.to_owned())),
        Fetched::Failed(said) => Err(CouldNotBuild(format!("{endpoint}: {said}"))),
    }
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

/// What GitHub says that a workload is made from.
struct Facts {
    branch: String,
    tip: String,
    issue: Value,
    /// The open tickets that block the issue.
    blocked_by: Vec<i64>,
    approver: String,
}

/// Read the default branch and its tip, the issue, and the signed-in user.
fn read(repository: &str, number: i64, approver: Option<&str>) -> Result<Facts, CouldNotBuild> {
    let base = format!("repos/{repository}");

    let repo = found(
        &base,
        &format!(
            "{repository}: GitHub says there is no such repository, or it is not visible to gh"
        ),
    )?;
    let branch = text(&repo, "default_branch");
    if branch.is_empty() {
        return Err(CouldNotBuild(format!("{repository} has no default branch")));
    }

    let tip_response = found(
        &format!("{base}/commits/{branch}"),
        &format!("{repository} has no commit on {branch}"),
    )?;
    let tip = text(&tip_response, "sha");
    if tip.len() != 40 || !tip.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(CouldNotBuild(format!(
            "GitHub did not give a commit for {branch}: {tip:?}"
        )));
    }

    let issue = found(
        &format!("{base}/issues/{number}"),
        &format!("there is no issue {number} in {repository}"),
    )?;
    if !issue["pull_request"].is_null() {
        return Err(CouldNotBuild(format!(
            "#{number} in {repository} is a pull request, not an issue"
        )));
    }
    let state = text(&issue, "state");
    if state != "open" {
        return Err(CouldNotBuild(format!(
            "#{number} in {repository} is not open (it is {state})"
        )));
    }

    let blocked_by = open_blockers(&base, number, &issue)?;

    // Who approved it is given, or is whoever is signed in.
    let approver = if let Some(login) = approver {
        login.to_owned()
    } else {
        let user = found("user", "GitHub does not say who is signed in to gh")?;
        let login = text(&user, "login");
        if login.is_empty() {
            return Err(CouldNotBuild(
                "GitHub did not give the signed-in user's login".to_owned(),
            ));
        }
        login
    };
    Ok(Facts {
        branch,
        tip,
        issue,
        blocked_by,
        approver,
    })
}

/// The numbers of the tickets that block the issue and are still open. GitHub says on
/// the issue how many block it, so the list is only asked for when there are some; one
/// that cannot be read is a reason and not a workload that does not know what blocks it.
fn open_blockers(base: &str, number: i64, issue: &Value) -> Result<Vec<i64>, CouldNotBuild> {
    if issue["issue_dependencies_summary"]["total_blocked_by"]
        .as_i64()
        .unwrap_or(0)
        == 0
    {
        return Ok(Vec::new());
    }
    let blockers = found(
        &format!("{base}/issues/{number}/dependencies/blocked_by"),
        &format!("GitHub has no list of what blocks #{number}"),
    )?;
    Ok(blockers
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|blocker| blocker["state"].as_str() == Some("open"))
        .filter_map(|blocker| blocker["number"].as_i64())
        .collect())
}

/// The number of the ticket this one is a sub-issue of, or zero.
fn parent_of(issue: &Value) -> i64 {
    issue["parent_issue_url"]
        .as_str()
        .and_then(|url| url.rsplit('/').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

/// A count GitHub gives on the issue, or zero.
fn count(issue: &Value, summary: &str, key: &str) -> i32 {
    issue[summary][key]
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(0)
}

/// Build the workload for a ticket, with the default branch's tip on GitHub as
/// the commit to start from, and the signed-in user as the one who approved it.
///
/// A run clones from GitHub, so it sees only what is pushed. `local_head`, if
/// the caller is in a checkout, is compared with the tip, and a difference is
/// a warning, not a refusal.
///
/// # Errors
///
/// Returns why a workload could not be built: GitHub could not be read, or the
/// ticket is not an open issue.
pub fn build(request: &Request<'_>, local_head: Option<&str>) -> Result<Built, CouldNotBuild> {
    let repository = request.repository;
    let (owner, name) = repository
        .split_once('/')
        .ok_or_else(|| CouldNotBuild(format!("{repository} is not OWNER/NAME")))?;
    let Facts {
        branch,
        tip,
        issue,
        blocked_by,
        approver,
    } = read(repository, request.ticket, request.approver)?;

    let mut warnings = Vec::new();
    if let Some(head) = local_head
        && head != tip
    {
        warnings.push(format!(
            "your checkout is at {}, not at GitHub's tip of {branch} ({}); the run uses the tip, and sees only what is pushed",
            &head[..head.len().min(7)],
            &tip[..7]
        ));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| CouldNotBuild(format!("the clock is before 1970: {e}")))?;
    let workload = Workload {
        id: uuid4()?,
        ticket: Some(Ticket {
            number: request.ticket,
            title: text(&issue, "title"),
            body: text(&issue, "body"),
            url: text(&issue, "html_url"),
            parent: parent_of(&issue),
            sub_issues: count(&issue, "sub_issues_summary", "total"),
            sub_issues_completed: count(&issue, "sub_issues_summary", "completed"),
            blocked_by,
        }),
        repository: Some(Repository {
            owner: owner.to_owned(),
            name: name.to_owned(),
            base_branch: branch,
            base_commit: tip,
            ..Repository::default()
        }),
        runtime: Some(AgentRuntime {
            kind: agent_runtime::Kind::ClaudeCode as i32,
            provider: agent_runtime::Provider::AnthropicSubscription as i32,
            model: request.model.to_owned(),
            connection: String::new(),
        }),
        limits: Some(Limits {
            timeout: Some(Duration {
                seconds: i64::try_from(request.timeout_seconds).unwrap_or(i64::MAX),
                nanos: 0,
            }),
            budget: Some(Budget {
                unit: CostUnit::Usd as i32,
                amount: request.budget_usd,
            }),
        }),
        goal_documents: Vec::new(),
        approval: Some(Approval {
            approved_by: approver,
            approved_at: Some(request.approved_at.unwrap_or(Timestamp {
                seconds: i64::try_from(now.as_secs()).unwrap_or(i64::MAX),
                nanos: 0,
            })),
        }),
        // A workload built from a ticket alone is never a revision.
        revision: None,
        review: None,
    };
    Ok(Built { workload, warnings })
}

/// A random version 4 UUID, from the system's source of randomness.
fn uuid4() -> Result<String, CouldNotBuild> {
    use std::fmt::Write as _;
    use std::io::Read as _;
    let mut bytes = [0_u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| CouldNotBuild(format!("could not get random bytes for an id: {e}")))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// The commit a checkout is at, if the path is a git checkout with one.
#[must_use]
pub fn head_of(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    let head = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && head.len() == 40).then_some(head)
}
