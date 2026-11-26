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
use heph::process::{Go, Streamed, Tail, Tick, stream_with_tail};

use crate::redact::Redactor;

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

/// Where the agent leaves a question when it cannot go on: one question, with the answer it
/// would choose and why. The run ends blocked with it, for a person to answer.
pub const QUESTION_FILE: &str = ".heph-question";

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
    // A prompt would wait for a person no run has, so a missing or refused credential must
    // fail at once instead.
    environment.retain(|(name, _)| name != "GIT_TERMINAL_PROMPT");
    environment.push(("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()));
    if !token.is_empty() {
        environment.push(("GH_TOKEN".to_owned(), token.to_owned()));
    }
    environment
}

/// How a git command that reaches the repository's host proves who it is: `gh`'s credential
/// helper, given the token. With no token the helper has nothing to give and the command
/// fails at once, with a reason that says so.
#[derive(Clone, Copy)]
pub struct Credential<'a> {
    /// The `gh` program the helper is.
    pub gh: &'a str,
    /// The token, if the run holds one.
    pub token: Option<&'a str>,
}

impl Credential<'_> {
    /// Run `git` with `args` in `directory`, authenticating with this credential and never
    /// prompting.
    ///
    /// # Errors
    ///
    /// Returns `what` and why git failed, naming the credential when there is none.
    pub fn git(
        self,
        args: &[&str],
        directory: &Path,
        what: &str,
        tick: &mut Tick<'_>,
    ) -> Result<(), String> {
        let helper = format!("credential.helper=!{} auth git-credential", self.gh);
        let mut command = vec!["git", "-c", "credential.helper=", "-c", &helper];
        command.extend(args);
        let token = self.token.unwrap_or_default();
        let what = if self.token.is_some() {
            what.to_owned()
        } else {
            format!("{what}: the run holds no GitHub token for the credential helper to give")
        };
        run_ticking(&command, directory, token, &what, tick).map(|_| ())
    }
}

/// How many of a command's last lines of standard error are kept, before the useful ones are
/// chosen.
const STDERR_LINES: usize = 40;

/// How many of those lines a failure reason holds, and how many characters of them.
const SAID_LINES: usize = 6;
const SAID_CHARS: usize = 800;

/// What a host said when it refused a command, fit for a failure reason: the last lines that say
/// something (git's `hint:` advice is left out), with every held secret replaced and the whole
/// capped. The token is taken out here as well as by the executor's own redaction, because the
/// reason is built before that runs.
fn host_said(stderr: &str, token: &str) -> String {
    let redactor = Redactor::new([("GITHUB_TOKEN", token)]);
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("hint:"))
        .collect();
    let kept = lines[lines.len().saturating_sub(SAID_LINES)..].join(" | ");
    let text = crate::run::for_the_log(&redactor.text(&kept));
    if text.chars().count() > SAID_CHARS {
        let cut: String = text.chars().take(SAID_CHARS).collect();
        format!("{cut}...")
    } else {
        text
    }
}

/// Run a command and return what it wrote to standard output, line by line. For the local ones,
/// which take no time worth a heartbeat.
fn run(command: &[&str], directory: &Path, token: &str, what: &str) -> Result<Vec<String>, String> {
    let mut nothing = || Go::On;
    let mut idle = Tick {
        every: TIMEOUT,
        call: &mut nothing,
    };
    run_ticking(command, directory, token, what, &mut idle)
}

/// [`run`] for a command that reaches the network. `tick` is called at its interval while the
/// command runs, and a tick that says to stop ends the command.
fn run_ticking(
    command: &[&str],
    directory: &Path,
    token: &str,
    what: &str,
    tick: &mut Tick<'_>,
) -> Result<Vec<String>, String> {
    let command: Vec<String> = command.iter().map(|word| (*word).to_owned()).collect();
    let lines = RefCell::new(Vec::new());
    let tail = Tail::new(STDERR_LINES);
    let ended = stream_with_tail(
        &command,
        directory,
        &environment(token),
        TIMEOUT,
        tick.every,
        |line| {
            lines.borrow_mut().push(line.to_owned());
            Go::On
        },
        || (tick.call)(),
        say,
        Some(&tail),
    )
    .map_err(|error| format!("{what}: {error}"))?;
    match ended {
        Streamed::Exited(0) => Ok(lines.into_inner()),
        Streamed::Exited(code) => {
            let said = host_said(&tail.text(), token);
            if said.is_empty() {
                Err(format!("{what}: exited with code {code}"))
            } else {
                Err(format!("{what}: exited with code {code}: {said}"))
            }
        }
        Streamed::TimedOut => Err(format!("{what}: timed out")),
        Streamed::Stopped => Err(format!("{what}: was stopped")),
        Streamed::Cancelled => Err(format!("{what}: cancelled")),
        Streamed::NotRun(_) => Err(format!("{what}: could not be started")),
        Streamed::InputFailed(reason) => {
            Err(format!("{what}: its input could not be written: {reason}"))
        }
    }
}

/// The largest file a delivery commits. Anything bigger is a dump or a build product left
/// behind, not source, and it would stay in the repository's history for good.
pub const LARGEST_FILE: u64 = 5 * 1024 * 1024;

/// What a BEAM that crashed leaves in the directory it ran in. It is a record of a stopped
/// run, never part of a change, so it is left out of the commit wherever it is.
const CRASH_DUMP: &str = "erl_crash.dump";

/// Stage the agent's work: everything except a crash dump, and nothing larger than
/// [`LARGEST_FILE`].
fn stage(workspace: &Path) -> Result<(), String> {
    let leave_out = format!(":(exclude,glob)**/{CRASH_DUMP}");
    run(
        &["git", "add", "-A", "--", ".", &leave_out],
        workspace,
        "",
        "git add",
    )?;
    let staged = run(
        &[
            "git",
            "-c",
            "core.quotepath=off",
            "diff",
            "--cached",
            "--name-only",
            "--diff-filter=ACMR",
        ],
        workspace,
        "",
        "git diff",
    )?;
    for name in staged {
        let size = std::fs::metadata(workspace.join(&name)).map_or(0, |m| m.len());
        if size > LARGEST_FILE {
            return Err(format!(
                "{name} is {size} bytes, over the {LARGEST_FILE}-byte limit for a committed file: delete it, or keep it out of the change"
            ));
        }
    }
    Ok(())
}

/// What a local commit made.
#[derive(Debug)]
pub struct Committed {
    /// The branch the commit is on, which is the one it will be pushed to.
    pub branch: String,
    /// Whether there is a commit to push. A revision that changed only the pull request's
    /// description has none: the branch is as the pull request already has it.
    pub made: std::cell::Cell<bool>,
}

/// The commit message the agent left, if it is one the repository's rule allows.
fn read_message(workspace: &Path) -> Result<String, String> {
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
    Ok(message)
}

/// Why the agent's commit message cannot be used, if it cannot: none was left, or the
/// repository's rule refuses it.
#[must_use]
pub fn message_problem(workspace: &Path) -> Option<String> {
    read_message(workspace).err()
}

/// Whether the working tree differs from the head.
///
/// # Errors
///
/// Returns why git could not say.
pub fn tree_changed(workspace: &Path) -> Result<bool, String> {
    Ok(!run(
        &["git", "status", "--porcelain"],
        workspace,
        "",
        "git status",
    )?
    .is_empty())
}

/// What a revision was for, as a line a commit message can carry.
fn revision_purpose(revision: &contracts::v1alpha1::Revision) -> String {
    if !revision.failures.is_empty() {
        let names: Vec<&str> = revision
            .failures
            .iter()
            .map(|failure| failure.name.as_str())
            .collect();
        format!("Revised after: {}.", names.join(", "))
    } else if revision.base_moved.is_some() {
        "Revised to confirm the change still stands on the base branch.".to_owned()
    } else {
        "Revised after a request for changes.".to_owned()
    }
}

/// Make sure a message the commit can use is in [`MESSAGE_FILE`]. A revision whose tree
/// changed and whose message is missing or refused is not thrown away: the pull request's own
/// title, which is the subject of its head commit, is used, with a line saying what the
/// revision was for. Any other workload's message is left as it is, for the caller to refuse.
///
/// # Errors
///
/// Returns why the fallback could not be written or the tree read.
pub fn settle_message(workload: &Workload, workspace: &Path) -> Result<(), String> {
    let Some(revision) = workload.revision.as_ref() else {
        return Ok(());
    };
    if read_message(workspace).is_ok() || !tree_changed(workspace)? {
        return Ok(());
    }
    let title = run(
        &["git", "log", "-1", "--format=%s"],
        workspace,
        "",
        "git log",
    )?
    .join(" ");
    let title = title.trim();
    let title = if is_conventional(title) {
        title
    } else {
        "fix: revise the change"
    };
    let message = format!("{title}\n\n{}\n", revision_purpose(revision));
    std::fs::write(workspace.join(MESSAGE_FILE), message)
        .map_err(|error| format!("could not write {MESSAGE_FILE}: {error}"))
}

/// Commit what the agent did, on the branch it will be pushed to. Nothing is pushed and no
/// token is held: this is the part of delivery that can be done, and read, before anything
/// leaves the machine.
///
/// # Errors
///
/// Returns why there is nothing to commit: no message, one that is not conventional, or
/// no change.
///
/// # Panics
///
/// If the workload has no ticket. Reading a workload refuses one without it.
pub fn commit_locally(workload: &Workload, workspace: &Path) -> Result<Committed, String> {
    let ticket = workload.ticket.clone().unwrap_or_default();
    settle_message(workload, workspace)?;
    let message = read_message(workspace)?;

    let tree_unchanged = run(
        &["git", "status", "--porcelain"],
        workspace,
        "",
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
            "",
            "git log",
        )?
        .join("\n")
        .trim()
            != message;
    // The description is kept out of the repository, so a revision that rewrote only it leaves
    // the tree untouched; it is still something to deliver, to the pull request and not to
    // its branch.
    let description_only = tree_unchanged
        && !account_only
        && workload.revision.is_some()
        && written_description(workspace).is_some();
    if tree_unchanged && !account_only && !description_only {
        return Err("the agent changed nothing, so there is nothing to deliver".to_owned());
    }

    // A revision goes on the pull request's own branch; any other workload on a
    // new one named for its ticket.
    let branch = workload.revision.as_ref().map_or_else(
        || branch_for(ticket.number, &ticket.title),
        |revision| revision.branch.clone(),
    );
    if description_only {
        return Ok(Committed {
            branch,
            made: std::cell::Cell::new(false),
        });
    }
    run(
        &["git", "switch", "-c", &branch],
        workspace,
        "",
        "git switch",
    )?;
    stage(workspace)?;
    let mut commit = vec!["git", "commit", "-q", "-F", MESSAGE_FILE, "--cleanup=strip"];
    if account_only {
        commit.push("--allow-empty");
    }
    run(&commit, workspace, "", "git commit")?;
    Ok(Committed {
        branch,
        made: std::cell::Cell::new(true),
    })
}

/// What the agent wrote in [`DESCRIPTION_FILE`], if it wrote anything.
fn written_description(workspace: &Path) -> Option<String> {
    std::fs::read_to_string(workspace.join(DESCRIPTION_FILE))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// Rewrite the commit that was made locally from what the working tree and the message now
/// are, so a change fixed before it is pushed is still one commit.
///
/// # Errors
///
/// Returns why the commit could not be rewritten.
pub fn amend_locally(committed: &Committed, workspace: &Path) -> Result<(), String> {
    read_message(workspace)?;
    stage(workspace)?;
    if !committed.made.get() {
        // Nothing was committed, so there is nothing of this run's to amend: the head is the
        // pull request's own, which is never rewritten. A fix that left the files and the
        // message as they were (it touched only the description, which is kept out of the
        // repository) is nothing to commit: an empty commit would dismiss approvals and
        // restart the checks for no change.
        let staged_something = !run(
            &["git", "diff", "--cached", "--name-only"],
            workspace,
            "",
            "git diff",
        )?
        .is_empty();
        let head_message = run(
            &["git", "log", "-1", "--format=%B"],
            workspace,
            "",
            "git log",
        )?
        .join("\n");
        if !staged_something && head_message.trim() == read_message(workspace)? {
            return Ok(());
        }
        run(
            &["git", "switch", "-c", &committed.branch],
            workspace,
            "",
            "git switch",
        )?;
        run(
            &[
                "git",
                "commit",
                "-q",
                "--allow-empty",
                "-F",
                MESSAGE_FILE,
                "--cleanup=strip",
            ],
            workspace,
            "",
            "git commit",
        )?;
        committed.made.set(true);
        return Ok(());
    }
    run(
        &[
            "git",
            "commit",
            "-q",
            "--amend",
            "--allow-empty",
            "-F",
            MESSAGE_FILE,
            "--cleanup=strip",
        ],
        workspace,
        "",
        "git commit --amend",
    )?;
    Ok(())
}

/// The branch name a push is tried on before any work is done. It is the run's own, under the
/// same `heph/` namespace as the branch the work will go on, and is never created: the push
/// that uses it is a dry run.
fn probe_branch(workload: &Workload) -> String {
    format!("heph/push-check-{}", workload.id)
}

/// The pull request's description, as the implementer wrote it, kept in the workspace where a
/// review reads it: the file the implementer wrote is never committed, so a review's fresh
/// clone does not hold it.
///
/// # Errors
///
/// Returns why the description could not be read or kept.
pub fn fetch_description(
    url: &str,
    workspace: &Path,
    gh: &str,
    token: &str,
    tick: &mut Tick<'_>,
) -> Result<(), String> {
    let mut text = current_description(url, workspace, gh, token, tick)?;
    text.push('\n');
    std::fs::write(workspace.join(DESCRIPTION_FILE), text)
        .map_err(|e| format!("could not keep the pull request's description: {e}"))
}

/// The description the pull request has now.
fn current_description(
    url: &str,
    workspace: &Path,
    gh: &str,
    token: &str,
    tick: &mut Tick<'_>,
) -> Result<String, String> {
    run_ticking(
        &[gh, "pr", "view", url, "--json", "body", "--jq", ".body"],
        workspace,
        token,
        "could not read the pull request's description",
        tick,
    )
    .map(|lines| lines.join("\n"))
}

/// Refuse a revision whose only delivery is a description that is the pull request's own,
/// word for word: it did nothing, and saying so is better than reporting a revision that
/// changed nothing as completed. A revision that made a commit has a change to deliver
/// whatever its description says.
///
/// # Errors
///
/// Returns why there is nothing to deliver, or why the current description could not be read.
pub fn refuse_unchanged_description(
    workload: &Workload,
    workspace: &Path,
    gh: &str,
    token: &str,
    deferred: &[Deferred],
    committed: &Committed,
    tick: &mut Tick<'_>,
) -> Result<(), String> {
    let Some(revision) = workload.revision.as_ref().filter(|_| !committed.made.get()) else {
        return Ok(());
    };
    let message = read_message(workspace)?;
    let resolution = read_resolution(workspace);
    let written = written_description(workspace);
    let description = Description {
        message: &message,
        written: written.as_deref(),
        deferred,
        resolution: &resolution,
    };
    let number = workload.ticket.as_ref().map_or(0, |ticket| ticket.number);
    let new = body(workload, &description, number);
    let current = current_description(&revision.pull_request_url, workspace, gh, token, tick)?;
    // The host keeps the line endings of whatever was typed in a browser, and a trailing
    // blank line means nothing.
    let same = |text: &str| {
        text.lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned()
    };
    if same(&new) == same(&current) {
        return Err(
            "the agent changed nothing, so there is nothing to deliver: the description it wrote is the pull request's own".to_owned(),
        );
    }
    Ok(())
}

/// Whether the token can push to the repository, found out before an agent starts and
/// anything is spent. It does a dry-run push of the base commit to a throwaway branch name,
/// the way and with the credential the real push will use. A dry run makes the host
/// authenticate the push and say what it would do, and sends no objects and updates no ref, so
/// nothing is written to the repository.
///
/// # Errors
///
/// Returns what the host said, and what to check, when the push is refused.
///
/// # Panics
///
/// If the workload has no repository. Reading a workload refuses one without it.
pub fn check_push(
    workload: &Workload,
    workspace: &Path,
    gh: &str,
    token: &str,
    tick: &mut Tick<'_>,
) -> Result<(), String> {
    let repository = workload.repository.as_ref().expect("checked when read");
    let helper = format!("credential.helper=!{gh} auth git-credential");
    let refspec = format!(
        "{}:refs/heads/{}",
        repository.base_commit,
        probe_branch(workload)
    );
    run_ticking(
        &[
            "git",
            "-c",
            "credential.helper=",
            "-c",
            &helper,
            "push",
            "--dry-run",
            "origin",
            &refspec,
        ],
        workspace,
        token,
        "the run cannot push to the repository, so it was not started",
        tick,
    )
    .map(|_| ())
    .map_err(|said| {
        format!(
            "{said}. Check that the token the run holds has write access to {}/{}.",
            repository.owner, repository.name
        )
    })
}

/// Push a commit that was made locally, and open the pull request, or for a revision update
/// the one that exists.
///
/// # Errors
///
/// Returns why it could not be pushed or the pull request opened or updated.
///
/// # Panics
///
/// If the workload has no ticket or repository. Reading a workload refuses one
/// without them.
pub fn push_and_open(
    workload: &Workload,
    workspace: &Path,
    gh: &str,
    token: &str,
    deferred: &[Deferred],
    committed: &Committed,
    tick: &mut Tick<'_>,
) -> Result<PullRequestOpened, String> {
    let message = read_message(workspace)?;
    let branch = committed.branch.clone();

    if committed.made.get() {
        let helper = format!("credential.helper=!{gh} auth git-credential");
        run_ticking(
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
            tick,
        )?;
    }

    let resolution = read_resolution(workspace);
    let written = written_description(workspace);
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
        update_pull_request(workload, workspace, gh, token, revision, &description, tick)?;
        return Ok(PullRequestOpened {
            url: revision.pull_request_url.clone(),
            number: revision.pull_request_number,
            head_branch: branch,
            nothing_pushed: !committed.made.get(),
        });
    }

    open_pull_request(workload, workspace, gh, token, &branch, &description, tick)
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
    tick: &mut Tick<'_>,
) -> Result<(), String> {
    let subject = description.message.lines().next().unwrap_or_default();
    let repository = workload.repository.as_ref().expect("checked when read");
    let repository_name = format!("{}/{}", repository.owner, repository.name);
    let body_path = write_body(workload, workspace, description)?;
    run_ticking(
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
        tick,
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
    tick: &mut Tick<'_>,
) -> Result<PullRequestOpened, String> {
    let subject = description.message.lines().next().unwrap_or_default();
    let repository = workload.repository.as_ref().expect("checked when read");
    let body_path = write_body(workload, workspace, description)?;
    let repository_name = format!("{}/{}", repository.owner, repository.name);
    let lines = run_ticking(
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
        tick,
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
        nothing_pushed: false,
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

/// The commit the agent says already fixes the ticket on the base branch, when
/// [`RESOLUTION_FILE`] says `resolved-by: <commit>`. Only a commit's hexadecimal name counts, so
/// text that is not one is never put in front of a person as a commit.
#[must_use]
pub fn resolved_by(workspace: &Path) -> Option<String> {
    let text = std::fs::read_to_string(workspace.join(RESOLUTION_FILE)).ok()?;
    let (word, rest) = text.trim().split_once(':')?;
    if !word.trim().eq_ignore_ascii_case("resolved-by") {
        return None;
    }
    let commit = rest.trim();
    ((7..=64).contains(&commit.len()) && commit.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| commit.to_owned())
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
    // Where the ticket is judged after the merge, the merge must not close it: a closed ticket is
    // not judged, so the agent's own claim would stand in place of the evidence.
    let judged = workload.judged_after_merge;
    if judged && complete && !epic {
        text.push_str(
            "The agent says this resolves the whole ticket. The ticket is judged once this merges, against what is on the default branch, and is closed then if the evidence decides.\n\n",
        );
    }
    let keyword = if complete && !epic && !judged {
        "Closes"
    } else {
        "Refs"
    };
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

#[cfg(test)]
mod tests {
    use super::host_said;

    #[test]
    fn what_the_host_said_has_the_token_taken_out_and_is_capped() {
        let token = "ghs_0123456789abcdef";
        let stderr = format!(
            "hint: some advice\nremote: Permission to a/b.git denied to c.\nfatal: unable to access 'https://x:{token}@github.com/a/b.git/': error: 403\n"
        );
        let said = host_said(&stderr, token);
        assert!(!said.contains(token), "{said}");
        assert!(said.contains("[redacted: GITHUB_TOKEN]"), "{said}");
        assert!(said.contains("Permission to a/b.git denied"), "{said}");
        assert!(!said.contains("hint:"), "{said}");

        let long = "x".repeat(5000);
        assert!(host_said(&long, token).chars().count() < 900);
    }
}
