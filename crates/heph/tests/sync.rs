//! `heph sync` as a person or a workflow meets it.
//!
//! Athenaeum is stood in for by a real git repository in a scratch directory,
//! with real tags, fetched through a `file://` URL. Nothing here needs a
//! network, and nothing touches the git configuration of whoever runs it.

mod common;

use std::path::Path;
use std::process::Command;

use common::{Scratch, heph_in, stderr, stdout};

/// Run git in a directory, ignoring the runner's own git configuration so
/// that signing or a missing identity cannot change what a test does.
fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git to run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output to be UTF-8")
        .trim()
        .to_owned()
}

/// A stand-in for Athenaeum: a git repository whose skills live in
/// `skills/<name>/`, tagged as it is released.
struct Athenaeum {
    directory: std::path::PathBuf,
}

impl Athenaeum {
    fn new(scratch: &Scratch) -> Self {
        let directory = scratch.0.join("upstream");
        std::fs::create_dir_all(&directory).expect("the upstream directory");
        git(&directory, &["init", "-q", "-b", "main"]);
        Self { directory }
    }

    fn url(&self) -> String {
        format!("file://{}", self.directory.display())
    }

    fn write(&self, relative: &str, text: &str) {
        let file = self.directory.join(relative);
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("the directory");
        std::fs::write(file, text).expect("the file");
    }

    /// Commit everything and tag it, returning the commit.
    fn release(&self, tag: &str) -> String {
        git(&self.directory, &["add", "-A"]);
        git(&self.directory, &["commit", "-q", "-m", tag]);
        git(&self.directory, &["tag", tag]);
        git(&self.directory, &["rev-parse", "HEAD"])
    }
}

/// An upstream with three skills, one of them with a nested file, released as
/// v0.1.0. Returns it with that release's commit.
fn upstream_with_skills(scratch: &Scratch) -> (Athenaeum, String) {
    let upstream = Athenaeum::new(scratch);
    upstream.write("skills/alpha/SKILL.md", "alpha, first release\n");
    upstream.write("skills/alpha/notes/detail.md", "nested detail\n");
    upstream.write("skills/beta/SKILL.md", "beta, first release\n");
    upstream.write("skills/gamma/SKILL.md", "gamma, first release\n");
    let commit = upstream.release("v0.1.0");
    (upstream, commit)
}

fn lock(url: &str, rev: &str, commit: &str, skills: &[&str], harnesses: &[&str]) -> String {
    let quote = |items: &[&str]| {
        items
            .iter()
            .map(|item| format!("{item:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "# Pinned by hand.\nschema_version = 0\n\nsource = {url:?}\nrev = {rev:?} # the release\ncommit = {commit:?}\nskills = [{}]\nharnesses = [{}]\n",
        quote(skills),
        quote(harnesses)
    )
}

fn repository(name: &str, lock: &str) -> Scratch {
    let scratch = Scratch::new(name);
    scratch.write(".athenaeum/lock.toml", lock);
    scratch
}

const LINK: &str = "../../.athenaeum/skills";

#[test]
fn it_vendors_the_skills_the_lock_names_and_links_them_for_claude_code() {
    let scratch = Scratch::new("installs");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "installs-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha", "beta"],
            &["claude-code"],
        ),
    );

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        repo.read(".athenaeum/skills/alpha/SKILL.md"),
        "alpha, first release\n"
    );
    assert_eq!(
        repo.read(".athenaeum/skills/alpha/notes/detail.md"),
        "nested detail\n"
    );
    assert_eq!(
        repo.read(".athenaeum/skills/beta/SKILL.md"),
        "beta, first release\n"
    );
    assert_eq!(
        repo.link_target(".claude/skills/alpha"),
        format!("{LINK}/alpha")
    );
    assert_eq!(
        repo.link_target(".claude/skills/beta"),
        format!("{LINK}/beta")
    );
    assert_eq!(
        repo.read(".claude/skills/alpha/SKILL.md"),
        "alpha, first release\n",
        "the link must resolve to the vendored copy"
    );
    for line in [
        "installed alpha",
        "installed beta",
        "linked alpha",
        "linked beta",
    ] {
        assert!(
            stdout(&output).contains(line),
            "stdout: {}",
            stdout(&output)
        );
    }
}

#[test]
fn only_the_skills_the_lock_names_are_vendored() {
    let scratch = Scratch::new("only-named");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "only-named-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha"],
            &["claude-code"],
        ),
    );

    heph_in(&repo.0, &["sync"]);

    assert!(repo.exists(".athenaeum/skills/alpha"));
    assert!(!repo.is_present(".athenaeum/skills/beta"));
    assert!(!repo.is_present(".claude/skills/gamma"));
}

#[test]
fn with_no_harnesses_nothing_is_linked() {
    let scratch = Scratch::new("no-harness");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "no-harness-repo",
        &lock(&upstream.url(), "v0.1.0", &commit, &["alpha"], &[]),
    );

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(repo.exists(".athenaeum/skills/alpha/SKILL.md"));
    assert!(!repo.is_present(".claude"));
}

#[test]
fn syncing_again_changes_nothing_and_says_so() {
    let scratch = Scratch::new("idempotent");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "idempotent-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha"],
            &["claude-code"],
        ),
    );
    heph_in(&repo.0, &["sync"]);

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "up to date\n");
}

#[test]
fn check_succeeds_on_a_repository_that_is_in_step() {
    let scratch = Scratch::new("check-clean");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "check-clean-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha"],
            &["claude-code"],
        ),
    );
    heph_in(&repo.0, &["sync"]);

    let output = heph_in(&repo.0, &["sync", "--check"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "up to date\n");
}

#[test]
fn check_reports_every_kind_of_difference_and_changes_nothing() {
    let scratch = Scratch::new("check-drift");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "check-drift-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha", "beta"],
            &["claude-code"],
        ),
    );
    heph_in(&repo.0, &["sync"]);

    repo.write(".athenaeum/skills/alpha/SKILL.md", "edited by hand\n");
    std::fs::remove_dir_all(repo.0.join(".athenaeum/skills/beta")).expect("remove beta");
    repo.write(
        ".athenaeum/skills/leftover/SKILL.md",
        "no longer in the lock\n",
    );
    std::fs::remove_file(repo.0.join(".claude/skills/alpha")).expect("remove the link");

    let output = heph_in(&repo.0, &["sync", "--check"]);

    assert_eq!(output.status.code(), Some(1));
    for difference in [
        "modified: alpha",
        "missing: beta",
        "extra: leftover",
        "not linked: alpha",
    ] {
        assert!(
            stderr(&output).contains(difference),
            "expected '{difference}' in: {}",
            stderr(&output)
        );
    }
    assert_eq!(
        repo.read(".athenaeum/skills/alpha/SKILL.md"),
        "edited by hand\n"
    );
    assert!(
        !repo.is_present(".athenaeum/skills/beta"),
        "check must not repair"
    );
    assert!(
        repo.is_present(".athenaeum/skills/leftover"),
        "check must not remove"
    );
    assert!(
        !repo.is_present(".claude/skills/alpha"),
        "check must not link"
    );
}

#[test]
fn sync_repairs_a_hand_edit_a_missing_skill_a_stale_skill_and_a_missing_link() {
    let scratch = Scratch::new("repairs");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "repairs-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha", "beta"],
            &["claude-code"],
        ),
    );
    heph_in(&repo.0, &["sync"]);
    repo.write(".athenaeum/skills/alpha/SKILL.md", "edited by hand\n");
    std::fs::remove_dir_all(repo.0.join(".athenaeum/skills/beta")).expect("remove beta");
    repo.write(
        ".athenaeum/skills/leftover/SKILL.md",
        "no longer in the lock\n",
    );
    std::os::unix::fs::symlink(
        format!("{LINK}/leftover"),
        repo.0.join(".claude/skills/leftover"),
    )
    .expect("a stale link");
    std::fs::remove_file(repo.0.join(".claude/skills/alpha")).expect("remove the link");

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        repo.read(".athenaeum/skills/alpha/SKILL.md"),
        "alpha, first release\n"
    );
    assert_eq!(
        repo.read(".athenaeum/skills/beta/SKILL.md"),
        "beta, first release\n"
    );
    assert!(!repo.is_present(".athenaeum/skills/leftover"));
    assert!(
        !repo.is_present(".claude/skills/leftover"),
        "its link goes too"
    );
    assert_eq!(
        repo.link_target(".claude/skills/alpha"),
        format!("{LINK}/alpha")
    );
    let report = stdout(&output);
    for line in [
        "updated alpha",
        "installed beta",
        "removed leftover",
        "unlinked leftover",
        "linked alpha",
    ] {
        assert!(report.contains(line), "expected '{line}' in: {report}");
    }
}

#[test]
fn a_link_that_is_not_into_the_vendored_skills_is_left_alone() {
    let scratch = Scratch::new("foreign-link");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "foreign-link-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha"],
            &["claude-code"],
        ),
    );
    repo.write("elsewhere/SKILL.md", "someone else's skill\n");
    std::fs::create_dir_all(repo.0.join(".claude/skills")).expect("claude skills");
    std::os::unix::fs::symlink("../../elsewhere", repo.0.join(".claude/skills/mine"))
        .expect("an unrelated link");

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(repo.link_target(".claude/skills/mine"), "../../elsewhere");
}

#[test]
fn it_refuses_when_the_tag_now_points_at_a_different_commit_and_writes_nothing() {
    let scratch = Scratch::new("moved-tag");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "moved-tag-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha"],
            &["claude-code"],
        ),
    );
    upstream.write("skills/alpha/SKILL.md", "rewritten after the release\n");
    git(&upstream.directory, &["add", "-A"]);
    git(&upstream.directory, &["commit", "-q", "-m", "rewrite"]);
    let moved_to = git(&upstream.directory, &["rev-parse", "HEAD"]);
    git(&upstream.directory, &["tag", "-f", "v0.1.0"]);

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains(&commit),
        "stderr: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains(&moved_to),
        "stderr: {}",
        stderr(&output)
    );
    assert!(
        !repo.is_present(".athenaeum/skills"),
        "nothing may be written"
    );
    assert!(!repo.is_present(".claude"), "nothing may be written");
}

#[test]
fn it_refuses_to_overwrite_a_directory_it_did_not_create_and_writes_nothing() {
    let scratch = Scratch::new("would-overwrite");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "would-overwrite-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha", "beta"],
            &["claude-code"],
        ),
    );
    repo.write(".claude/skills/beta/SKILL.md", "a copy of my own\n");

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains(".claude/skills/beta"),
        "stderr: {}",
        stderr(&output)
    );
    assert_eq!(
        repo.read(".claude/skills/beta/SKILL.md"),
        "a copy of my own\n"
    );
    assert!(
        !repo.is_present(".athenaeum/skills"),
        "alpha must not have been installed before the conflict was found"
    );
    assert!(!repo.is_present(".claude/skills/alpha"));
}

#[test]
fn a_skill_the_source_does_not_have_could_not_be_synced() {
    let scratch = Scratch::new("no-such-skill");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "no-such-skill-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha", "delta"],
            &["claude-code"],
        ),
    );

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains("delta"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(!repo.is_present(".athenaeum/skills"));
}

#[test]
fn a_source_that_cannot_be_fetched_could_not_be_synced_and_says_what_git_said() {
    let scratch = Scratch::new("bad-source");
    let repo = repository(
        "bad-source-repo",
        &lock(
            &format!("file://{}/no-such-repository", scratch.0.display()),
            "v0.1.0",
            &"0".repeat(40),
            &["alpha"],
            &["claude-code"],
        ),
    );

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains("git"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_missing_lock_could_not_be_synced_and_the_error_names_the_file() {
    let scratch = Scratch::new("no-lock");

    let output = heph_in(&scratch.0, &["sync"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains(".athenaeum/lock.toml"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_invalid_lock_could_not_be_synced_and_the_error_says_where() {
    let scratch = Scratch::new("invalid-lock");
    let repo = repository(
        "invalid-lock-repo",
        &lock(
            "https://example.invalid/athenaeum",
            "main",
            &"a".repeat(40),
            &["alpha"],
            &[],
        ),
    );
    drop(scratch);

    let output = heph_in(&repo.0, &["sync"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains("$.rev"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn the_lock_can_be_named() {
    let scratch = Scratch::new("named-lock");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = Scratch::new("named-lock-repo");
    repo.write(
        "elsewhere/lock.toml",
        &lock(&upstream.url(), "v0.1.0", &commit, &["alpha"], &[]),
    );

    let output = heph_in(&repo.0, &["sync", "--lock", "elsewhere/lock.toml"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(repo.exists(".athenaeum/skills/alpha/SKILL.md"));
}

#[test]
fn update_moves_the_pin_to_the_latest_release_keeping_the_locks_comments() {
    let scratch = Scratch::new("update-latest");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "update-latest-repo",
        &lock(
            &upstream.url(),
            "v0.1.0",
            &commit,
            &["alpha"],
            &["claude-code"],
        ),
    );
    heph_in(&repo.0, &["sync"]);
    upstream.write("skills/alpha/SKILL.md", "alpha, second release\n");
    let second = upstream.release("v0.2.0");
    upstream.write("skills/alpha/SKILL.md", "alpha, tenth release\n");
    let tenth = upstream.release("v0.10.0");

    let output = heph_in(&repo.0, &["sync", "--update"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let written = repo.read(".athenaeum/lock.toml");
    assert!(
        written.contains(&format!("commit = \"{tenth}\"")),
        "{written}"
    );
    assert!(
        written.contains("rev = \"v0.10.0\" # the release"),
        "{written}"
    );
    assert!(written.starts_with("# Pinned by hand."), "{written}");
    assert!(!written.contains(&second));
    assert_eq!(
        repo.read(".athenaeum/skills/alpha/SKILL.md"),
        "alpha, tenth release\n",
        "v0.10.0 is later than v0.2.0, so the numbers must be compared as numbers"
    );
    assert!(
        stdout(&output).contains("pinned v0.10.0"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn update_can_move_the_pin_to_a_named_release() {
    let scratch = Scratch::new("update-named");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let repo = repository(
        "update-named-repo",
        &lock(&upstream.url(), "v0.1.0", &commit, &["alpha"], &[]),
    );
    upstream.write("skills/alpha/SKILL.md", "alpha, second release\n");
    let second = upstream.release("v0.2.0");
    upstream.write("skills/alpha/SKILL.md", "alpha, third release\n");
    upstream.release("v0.3.0");

    let output = heph_in(&repo.0, &["sync", "--update", "v0.2.0"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        repo.read(".athenaeum/lock.toml")
            .contains(&format!("commit = \"{second}\""))
    );
    assert_eq!(
        repo.read(".athenaeum/skills/alpha/SKILL.md"),
        "alpha, second release\n"
    );
}

#[test]
fn update_to_a_release_that_does_not_exist_changes_nothing() {
    let scratch = Scratch::new("update-missing");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let before = lock(&upstream.url(), "v0.1.0", &commit, &["alpha"], &[]);
    let repo = repository("update-missing-repo", &before);

    let output = heph_in(&repo.0, &["sync", "--update", "v9.9.9"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(stderr(&output).contains("v9.9.9"), "{}", stderr(&output));
    assert_eq!(repo.read(".athenaeum/lock.toml"), before);
    assert!(!repo.is_present(".athenaeum/skills"));
}

#[test]
fn update_does_not_write_the_lock_if_syncing_would_overwrite_something() {
    let scratch = Scratch::new("update-conflict");
    let (upstream, commit) = upstream_with_skills(&scratch);
    let before = lock(
        &upstream.url(),
        "v0.1.0",
        &commit,
        &["alpha"],
        &["claude-code"],
    );
    let repo = repository("update-conflict-repo", &before);
    repo.write(".claude/skills/alpha/SKILL.md", "a copy of my own\n");
    upstream.write("skills/alpha/SKILL.md", "alpha, second release\n");
    upstream.release("v0.2.0");

    let output = heph_in(&repo.0, &["sync", "--update"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        repo.read(".athenaeum/lock.toml"),
        before,
        "a refusal must leave the lock as it was"
    );
}

#[test]
fn a_command_line_that_cannot_be_understood_is_a_usage_error() {
    let scratch = Scratch::new("usage");

    for args in [
        &["sync", "extra"][..],
        &["sync", "--lock"],
        &["sync", "--nonsense"],
        &["sync", "--check", "--update"],
        &["sync", "--update", "v0.1.0", "v0.2.0"],
    ] {
        let output = heph_in(&scratch.0, args);

        assert_eq!(output.status.code(), Some(2), "for {args:?}");
        assert_eq!(stdout(&output), "", "for {args:?}");
        assert!(stderr(&output).contains("Usage: heph "), "for {args:?}");
    }
}
