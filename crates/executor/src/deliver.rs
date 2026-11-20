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
use std::path::{Path, PathBuf};
use std::time::Duration;

use contracts::v1alpha1::{PullRequestOpened, Workload};
use heph::process::{Go, Streamed, stream};

use crate::run::say;

/// Where the agent leaves the commit message, at the workspace root. The
/// executor keeps it out of the workspace's changes.
pub const MESSAGE_FILE: &str = ".heph-commit-message";

/// Where the agent says whether its change resolves the whole ticket: `complete`, or
/// `partial` and what remains. A ticket is closed only when every item that affects it
/// is resolved, so only `complete` lets the pull request close it.
pub const RESOLUTION_FILE: &str = ".heph-resolution";

/// Where the agent leaves the pull request's description, apart from the commit message:
/// what the ticket asked for, what the change does and the risks. It is optional, and the
/// commit message's body is the description when there is none.
pub const DESCRIPTION_FILE: &str = ".heph-description.md";

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
    deferred: &[Deferred],
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

    let tree_unchanged = run(
        &["git", "status", "--porcelain"],
        workspace,
        token,
        "git status",
    )?
    .is_empty();
    // A review reads the commit message as part of the change, so a revision that only
    // corrects what the change says about itself is delivered as a commit with no file
    // changes. A revision that says what the head already says did nothing, and an empty
    // commit would only restart the checks.
    let account_only = tree_unchanged
        && workload.revision.is_some()
        && run(
            &["git", "log", "-1", "--format=%B"],
            workspace,
            token,
            "git log",
        )?
        .join("\n")
        .trim()
            != message;
    if tree_unchanged && !account_only {
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
    let mut commit = vec!["git", "commit", "-q", "-F", MESSAGE_FILE, "--cleanup=strip"];
    if account_only {
        commit.push("--allow-empty");
    }
    run(&commit, workspace, token, "git commit")?;

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

    let resolution = read_resolution(workspace);
    let written = std::fs::read_to_string(workspace.join(DESCRIPTION_FILE))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    let description = Description {
        message: &message,
        written: written.as_deref(),
        deferred,
        resolution: &resolution,
    };

    // A revision's pull request exists, and the push above put the work on it. The push
    // is a plain one, so a branch that has moved is refused, never forced. The
    // description is then made from this revision's own account, so it cannot go on
    // saying what an earlier head did: the reviews read it as part of the change. It is
    // done after the push, so a push that was refused leaves the description describing
    // the head the pull request still has.
    if let Some(revision) = workload.revision.as_ref() {
        update_pull_request(workload, workspace, gh, token, revision, &description)?;
        return Ok(PullRequestOpened {
            url: revision.pull_request_url.clone(),
            number: revision.pull_request_number,
            head_branch: branch,
        });
    }

    open_pull_request(workload, workspace, gh, token, &branch, &description)
}

/// Write the description of a pull request to a file beside the workspace, for `gh`.
fn write_body(
    workload: &Workload,
    workspace: &Path,
    description: &Description<'_>,
) -> Result<PathBuf, String> {
    let ticket = workload.ticket.clone().unwrap_or_default();
    let body_path = workspace.with_file_name(format!("{}.pr-body.md", workload.id));
    std::fs::write(&body_path, body(workload, description, ticket.number))
        .map_err(|e| format!("could not write the pull request's body: {e}"))?;
    Ok(body_path)
}

/// Set the title and description of the pull request a revision was pushed to.
fn update_pull_request(
    workload: &Workload,
    workspace: &Path,
    gh: &str,
    token: &str,
    revision: &contracts::v1alpha1::Revision,
    description: &Description<'_>,
) -> Result<(), String> {
    let subject = description.message.lines().next().unwrap_or_default();
    let repository = workload.repository.as_ref().expect("checked when read");
    let repository_name = format!("{}/{}", repository.owner, repository.name);
    let body_path = write_body(workload, workspace, description)?;
    run(
        &[
            gh,
            "pr",
            "edit",
            &revision.pull_request_number.to_string(),
            "--repo",
            &repository_name,
            "--title",
            subject,
            "--body-file",
            &body_path.display().to_string(),
        ],
        workspace,
        token,
        "the work was pushed, but could not update the pull request",
    )?;
    Ok(())
}

/// Open the pull request for a branch that was pushed.
fn open_pull_request(
    workload: &Workload,
    workspace: &Path,
    gh: &str,
    token: &str,
    branch: &str,
    description: &Description<'_>,
) -> Result<PullRequestOpened, String> {
    let subject = description.message.lines().next().unwrap_or_default();
    let repository = workload.repository.as_ref().expect("checked when read");
    let body_path = write_body(workload, workspace, description)?;
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

/// What the agent said about how much of the ticket its change resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Everything the ticket asks, and nothing remains: the pull request may close it.
    Complete,
    /// Some of it. What remains, if the agent said.
    Partial(Option<String>),
    /// The agent did not say, or said something else: the pull request only refers to the
    /// ticket.
    Unsaid,
}

/// Read what the agent left in [`RESOLUTION_FILE`]. Only the single word `complete`, in
/// any case and with any whitespace around it, is a claim that the ticket is resolved:
/// anything else (more words, another word, an empty file, no file) is not.
fn read_resolution(workspace: &Path) -> Resolution {
    let Ok(text) = std::fs::read_to_string(workspace.join(RESOLUTION_FILE)) else {
        return Resolution::Unsaid;
    };
    let text = text.trim();
    if text.eq_ignore_ascii_case("complete") {
        return Resolution::Complete;
    }
    let (word, rest) = text.split_once(':').unwrap_or((text, ""));
    if word.trim().eq_ignore_ascii_case("partial") {
        let rest = rest.trim();
        return Resolution::Partial((!rest.is_empty()).then(|| rest.to_owned()));
    }
    Resolution::Unsaid
}

/// What goes in the pull request's description besides the agent's message.
struct Description<'a> {
    message: &'a str,
    /// What the agent wrote in the description file, if it wrote one.
    written: Option<&'a str>,
    deferred: &'a [Deferred],
    resolution: &'a Resolution,
}

/// A gate the workspace could not run and left to a check on the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deferred {
    /// The gate's name in the manifest.
    pub gate: String,
    /// Why it was not run.
    pub reason: String,
    /// The check that the repository says exercises the same work.
    pub ci_check: String,
}

/// The pull request's body: the message's body, what the workspace could not run
/// and left to the pull request's checks, what the agent says is not resolved, who
/// approved the ticket and when, and a last line that closes the ticket or only refers
/// to it.
fn body(workload: &Workload, description: &Description<'_>, number: i64) -> String {
    let Description {
        message,
        written,
        deferred,
        resolution,
    } = description;
    // The description the agent wrote is the pull request's account; without one, the
    // commit message's body is.
    let rest =
        written.unwrap_or_else(|| message.split_once('\n').map_or("", |(_, rest)| rest.trim()));
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
    if !deferred.is_empty() {
        text.push_str(
            "Gates not run in the workspace, left to the checks on this pull request:\n\n",
        );
        for left in *deferred {
            let _ = writeln!(
                text,
                "- `{}`: {}. The repository says the check `{}` runs the same work.",
                left.gate, left.reason, left.ci_check
            );
        }
        text.push('\n');
    }
    // The last line closes the ticket only when the agent says it resolves all of it.
    // Otherwise it refers to the ticket, and says what the agent said remains.
    if let Resolution::Partial(remaining) = resolution {
        let _ = writeln!(
            text,
            "## What remains\n\n{}\n",
            remaining
                .as_deref()
                .unwrap_or("the agent did not say what remains")
        );
    }
    // An epic is never closed by a pull request: what its parts add up to is for a person
    // to judge complete. One that resolves everything the agent was asked on it says so.
    let epic = workload.ticket.as_ref().is_some_and(|t| t.sub_issues > 0);
    let complete = matches!(resolution, Resolution::Complete);
    if epic && complete {
        text.push_str(
            "This ticket is an epic, so a pull request does not close it: a person judges it complete.\n\n",
        );
    }
    let keyword = if complete && !epic { "Closes" } else { "Refs" };
    let _ = write!(
        text,
        "Approved by {} at {when}.\n\n{keyword} #{number}\n",
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
