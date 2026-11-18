//! Delivery: commit the agent's work, push it to a branch and open the pull
//! request. For a revision the branch is the pull request's own and nothing is
//! opened.
//!
//! This is the only step that holds the token that can push, and it runs after
//! the agent has ended and the gates have passed. The token reaches `git push`
//! through `gh`'s credential helper and `gh pr create` as `GH_TOKEN`, and is
//! never on a command line.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use contracts::v1alpha1::{PullRequestOpened, Workload};
use heph::process::{Go, Streamed, stream};

use crate::run::say;

/// Where the agent leaves the commit message, at the workspace root. The
/// executor keeps it out of the workspace's changes.
pub const MESSAGE_FILE: &str = ".heph-commit-message";

/// How long one git or gh command may take.
const TIMEOUT: Duration = Duration::from_secs(600);

/// The commit types the repository's conventional-commit rule allows.
const TYPES: [&str; 10] = [
    "feat", "fix", "docs", "chore", "ci", "test", "refactor", "perf", "build", "style",
];

/// Whether a subject is `type(scope)!: description`, as the repository's commit
/// lint requires, because the squash-merge title is the commit and the release
/// version is computed from it.
#[must_use]
pub fn is_conventional(subject: &str) -> bool {
    let Some((head, description)) = subject.split_once(": ") else {
        return false;
    };
    if description.trim().is_empty() {
        return false;
    }
    let head = head.strip_suffix('!').unwrap_or(head);
    let kind = match head.split_once('(') {
        Some((kind, scope)) => {
            let Some(scope) = scope.strip_suffix(')') else {
                return false;
            };
            if scope.is_empty()
                || !scope
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                return false;
            }
            kind
        }
        None => head,
    };
    TYPES.contains(&kind)
}

/// The branch a workload's work goes on: `heph/<ticket number>-<slug of the
/// ticket's title>`.
#[must_use]
pub fn branch_for(number: i64, title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(40).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        format!("heph/{number}")
    } else {
        format!("heph/{number}-{slug}")
    }
}

/// The environment git and gh get: what they need to find programs and the
/// maintainer's git identity, and the token. Nothing else of this process's.
fn environment(token: &str) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = std::env::vars()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "PATH" | "HOME" | "USER" | "LANG" | "LC_ALL" | "TERM" | "TMPDIR"
            ) || name.starts_with("GIT_")
        })
        .collect();
    environment.push(("GH_TOKEN".to_owned(), token.to_owned()));
    environment
}

/// Run a command and return what it wrote to standard output, line by line.
fn run(command: &[&str], directory: &Path, token: &str, what: &str) -> Result<Vec<String>, String> {
    let command: Vec<String> = command.iter().map(|word| (*word).to_owned()).collect();
    let lines = RefCell::new(Vec::new());
    let ended = stream(
        &command,
        directory,
        &environment(token),
        TIMEOUT,
        TIMEOUT,
        |line| {
            lines.borrow_mut().push(line.to_owned());
            Go::On
        },
        || Go::On,
        say,
    )
    .map_err(|error| format!("{what}: {error}"))?;
    match ended {
        Streamed::Exited(0) => Ok(lines.into_inner()),
        Streamed::Exited(code) => Err(format!("{what}: exited with code {code}")),
        Streamed::TimedOut => Err(format!("{what}: timed out")),
        Streamed::Stopped => Err(format!("{what}: was stopped")),
        Streamed::Cancelled => Err(format!("{what}: cancelled")),
        Streamed::NotRun(_) => Err(format!("{what}: could not be started")),
    }
}

/// Commit what the agent did, push it, and open the pull request.
///
/// # Errors
///
/// Returns why nothing could be delivered, or why delivery stopped part way:
/// the reason the workload ends failed. Nothing already done is undone.
///
/// # Panics
///
/// If the workload has no ticket or repository. Reading a workload refuses one
/// without them.
pub fn deliver(
    workload: &Workload,
    workspace: &Path,
    gh: &str,
    token: &str,
) -> Result<PullRequestOpened, String> {
    let ticket = workload.ticket.clone().unwrap_or_default();

    let message_path = workspace.join(MESSAGE_FILE);
    let message = std::fs::read_to_string(&message_path).map_err(|_| {
        format!("the agent left no commit message: it must write a conventional commit subject to {MESSAGE_FILE}")
    })?;
    let message = message.trim().to_owned();
    let subject = message.lines().next().unwrap_or_default().to_owned();
    if !is_conventional(&subject) {
        return Err(format!(
            "{MESSAGE_FILE} does not start with a conventional commit subject (type: description): {subject:?}"
        ));
    }

    if run(
        &["git", "status", "--porcelain"],
        workspace,
        token,
        "git status",
    )?
    .is_empty()
    {
        return Err("the agent changed nothing, so there is nothing to deliver".to_owned());
    }

    // A revision goes on the pull request's own branch; any other workload on a
    // new one named for its ticket.
    let branch = workload.revision.as_ref().map_or_else(
        || branch_for(ticket.number, &ticket.title),
        |revision| revision.branch.clone(),
    );
    run(
        &["git", "switch", "-c", &branch],
        workspace,
        token,
        "git switch",
    )?;
    run(&["git", "add", "-A"], workspace, token, "git add")?;
    run(
        &["git", "commit", "-q", "-F", MESSAGE_FILE, "--cleanup=strip"],
        workspace,
        token,
        "git commit",
    )?;

    let helper = format!("credential.helper=!{gh} auth git-credential");
    run(
        &[
            "git",
            "-c",
            "credential.helper=",
            "-c",
            &helper,
            "push",
            "origin",
            &branch,
        ],
        workspace,
        token,
        &format!("could not push {branch}"),
    )?;

    // A revision's pull request exists: pushing to its branch is all there is to do.
    // The push above is a plain one, so a branch that has moved is refused, never
    // forced.
    if let Some(revision) = workload.revision.as_ref() {
        return Ok(PullRequestOpened {
            url: revision.pull_request_url.clone(),
            number: revision.pull_request_number,
            head_branch: branch,
        });
    }

    open_pull_request(workload, workspace, gh, token, &branch, &message, &subject)
}

/// Open the pull request for a branch that was pushed.
fn open_pull_request(
    workload: &Workload,
    workspace: &Path,
    gh: &str,
    token: &str,
    branch: &str,
    message: &str,
    subject: &str,
) -> Result<PullRequestOpened, String> {
    let repository = workload.repository.as_ref().expect("checked when read");
    let ticket = workload.ticket.clone().unwrap_or_default();
    let body_path = workspace.with_file_name(format!("{}.pr-body.md", workload.id));
    std::fs::write(&body_path, body(workload, message, ticket.number))
        .map_err(|e| format!("could not write the pull request's body: {e}"))?;
    let repository_name = format!("{}/{}", repository.owner, repository.name);
    let lines = run(
        &[
            gh,
            "pr",
            "create",
            "--repo",
            &repository_name,
            "--head",
            branch,
            "--base",
            &repository.base_branch,
            "--title",
            subject,
            "--body-file",
            &body_path.display().to_string(),
        ],
        workspace,
        token,
        "could not open the pull request",
    )?;

    let url = lines
        .iter()
        .rev()
        .find(|line| line.starts_with("http"))
        .cloned()
        .ok_or("could not open the pull request: gh did not print its address")?;
    let number = url
        .rsplit('/')
        .next()
        .and_then(|last| last.parse::<i64>().ok())
        .ok_or_else(|| format!("could not open the pull request: no number in {url}"))?;
    Ok(PullRequestOpened {
        url,
        number,
        head_branch: branch.to_owned(),
    })
}

/// The pull request's body: the message's body, who approved the ticket and
/// when, and a last line that closes the ticket.
fn body(workload: &Workload, message: &str, number: i64) -> String {
    let rest = message.split_once('\n').map_or("", |(_, rest)| rest.trim());
    let approval = workload.approval.clone().unwrap_or_default();
    let when = approval
        .approved_at
        .map(|t| rfc3339(t.seconds))
        .unwrap_or_default();
    let mut text = String::new();
    if !rest.is_empty() {
        text.push_str(rest);
        text.push_str("\n\n");
    }
    let _ = write!(
        text,
        "Approved by {} at {when}.\n\nCloses #{number}\n",
        approval.approved_by
    );
    text
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
fn rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}
