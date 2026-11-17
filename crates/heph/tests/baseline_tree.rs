//! What the baseline check reads of a working tree: what the repository
//! contains, and not what happens to be lying in its directory.
//!
//! A working tree can hold another checkout of the same repository (a git
//! worktree, a vendored copy) and anything git ignores. None of that is the
//! repository, so none of it may be read as an ecosystem or satisfy an item.

mod common;

use std::path::Path;
use std::process::Command;

use common::{Scratch, heph_in, stderr, stdout};
use serde_json::Value;

const COVERAGE: &str = "dependencies-dependabot-coverage";

fn git(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(directory)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
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
}

fn check(directory: &Path, extra: &[&str]) -> Value {
    let mut args = vec!["baseline", "check"];
    args.extend_from_slice(extra);
    let output = heph_in(directory, &args);
    serde_json::from_str(&stdout(&output))
        .unwrap_or_else(|error| panic!("JSON ({error}): {}", stderr(&output)))
}

fn outcome<'a>(result: &'a Value, item: &str) -> &'a str {
    result["report"]["items"][item]["outcome"]
        .as_str()
        .unwrap_or_else(|| panic!("no outcome for {item}: {result}"))
}

fn detail(result: &Value, item: &str) -> String {
    result["report"]["items"][item]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// A git repository with a Rust workspace at its root, committed.
fn repository(name: &str) -> Scratch {
    let scratch = Scratch::new(name);
    git(&scratch.0, &["init", "-q"]);
    scratch.write("Cargo.toml", "[workspace]\n");
    scratch.write("Cargo.lock", "# lock\n");
    git(&scratch.0, &["add", "."]);
    git(&scratch.0, &["commit", "-q", "-m", "first"]);
    scratch
}

#[test]
fn a_git_worktree_inside_the_tree_is_not_part_of_the_repository() {
    // What happened on a real checkout: an agent worktree under
    // .claude/worktrees, excluded by .git/info/exclude, holding a whole second
    // copy of the repository.
    let scratch = repository("bt-worktree");
    scratch.write("package-lock.json", "{}\n");
    git(&scratch.0, &["add", "."]);
    git(&scratch.0, &["commit", "-q", "-m", "more"]);
    git(
        &scratch.0,
        &[
            "worktree",
            "add",
            "-q",
            ".claude/worktrees/agent-1",
            "-b",
            "agent-1",
        ],
    );
    scratch.write(".git/info/exclude", ".claude/worktrees/\n");
    scratch.write(".claude/worktrees/agent-1/e2e/package-lock.json", "{}\n");

    let result = check(&scratch.0, &[]);

    let message = detail(&result, COVERAGE);
    assert!(
        message.contains("cargo /") && message.contains("npm /"),
        "{message}"
    );
    assert!(
        !message.contains(".claude"),
        "a worktree was read: {message}"
    );
}

#[test]
fn a_directory_that_git_ignores_is_not_part_of_the_repository() {
    let scratch = repository("bt-ignored");
    scratch.write(".gitignore", "vendor-copy/\n");
    scratch.write("vendor-copy/Cargo.lock", "# lock\n");
    scratch.write("vendor-copy/package-lock.json", "{}\n");

    let result = check(&scratch.0, &[]);

    let message = detail(&result, COVERAGE);
    assert!(message.contains("cargo /"), "{message}");
    assert!(
        !message.contains("vendor-copy"),
        "an ignored directory was read: {message}"
    );
}

#[test]
fn an_ignored_file_does_not_satisfy_an_item() {
    let scratch = repository("bt-ignored-file");
    scratch.write(".gitignore", "README.md\n");
    scratch.write("README.md", "# not committed, and never will be\n");

    let result = check(&scratch.0, &[]);

    assert_eq!(
        outcome(&result, "community-readme"),
        "unsatisfied",
        "{result}"
    );
}

#[test]
fn a_file_that_is_tracked_or_new_but_not_ignored_counts_and_a_deleted_one_does_not() {
    let scratch = repository("bt-counts");
    scratch.write("README.md", "# tracked\n");
    scratch.write("SECURITY.md", "# tracked, then deleted\n");
    git(&scratch.0, &["add", "."]);
    git(&scratch.0, &["commit", "-q", "-m", "docs"]);
    std::fs::remove_file(scratch.0.join("SECURITY.md")).expect("to delete");
    scratch.write("CONTRIBUTING.md", "# new, not yet added\n");

    let result = check(&scratch.0, &[]);

    assert_eq!(outcome(&result, "community-readme"), "satisfied", "tracked");
    assert_eq!(
        outcome(&result, "community-contributing"),
        "satisfied",
        "new"
    );
    assert_eq!(
        outcome(&result, "community-security-policy"),
        "unsatisfied",
        "deleted from the disk, though git still lists it"
    );
}

#[test]
fn a_nested_checkout_is_skipped_even_when_the_directory_is_not_a_git_repository() {
    // Without git to ask, the walk still must not read another checkout: a
    // directory with its own .git, whether a directory or a file.
    let scratch = Scratch::new("bt-nested-plain");
    scratch.write("Cargo.toml", "[package]\n");
    scratch.write("copy/.git", "gitdir: /elsewhere\n");
    scratch.write("copy/Cargo.lock", "# lock\n");
    scratch.write("copy/package-lock.json", "{}\n");
    scratch.write("other/.git/HEAD", "ref: refs/heads/main\n");
    scratch.write("other/package-lock.json", "{}\n");

    let result = check(&scratch.0, &[]);

    let message = detail(&result, COVERAGE);
    assert!(message.contains("cargo /"), "{message}");
    assert!(
        !message.contains("copy") && !message.contains("other") && !message.contains("npm"),
        "a nested checkout was read: {message}"
    );
}

#[test]
fn a_directory_that_is_not_a_git_repository_is_read_as_it_always_was() {
    let scratch = Scratch::new("bt-plain");
    scratch.write("Cargo.toml", "[package]\n");
    scratch.write("README.md", "# hello\n");
    scratch.write("app/package-lock.json", "{}\n");

    let result = check(&scratch.0, &[]);

    assert_eq!(outcome(&result, "community-readme"), "satisfied");
    let message = detail(&result, COVERAGE);
    assert!(
        message.contains("cargo /") && message.contains("npm /app"),
        "{message}"
    );
}

#[test]
fn a_root_inside_a_repository_reads_only_what_is_under_it() {
    let scratch = repository("bt-subroot");
    scratch.write("app/package-lock.json", "{}\n");
    scratch.write("app/README.md", "# app\n");
    scratch.write("elsewhere/Dockerfile", "FROM scratch\n");
    git(&scratch.0, &["add", "."]);
    git(&scratch.0, &["commit", "-q", "-m", "app"]);

    let result = check(&scratch.0, &["--root", "app"]);

    assert_eq!(
        outcome(&result, "community-readme"),
        "satisfied",
        "{result}"
    );
    let message = detail(&result, COVERAGE);
    assert!(message.contains("npm /"), "{message}");
    assert!(
        !message.contains("cargo") && !message.contains("docker"),
        "something outside the root was read: {message}"
    );
}

#[test]
fn apply_does_not_generate_entries_for_a_nested_checkout_or_an_ignored_directory() {
    let scratch = repository("bt-apply");
    scratch.write(".gitignore", "vendor-copy/\n");
    scratch.write("vendor-copy/Cargo.lock", "# lock\n");
    scratch.write("copy/.git", "gitdir: /elsewhere\n");
    scratch.write("copy/package-lock.json", "{}\n");
    scratch.write(
        ".athenaeum/baseline.toml",
        "schema_version = 0\nlicence = \"source-available\"\nholder = \"H\"\n\
         contact = \"https://example.org/c\"\nrequired_checks = [\"ci\"]\n\
         default_branch_bypass = \"none\"\n",
    );

    let output = heph_in(&scratch.0, &["baseline", "apply", "--repo", "owner/name"]);

    assert!(
        scratch.exists(".github/dependabot.yml"),
        "stderr: {}",
        stderr(&output)
    );
    let text = scratch.read(".github/dependabot.yml");
    assert!(text.contains("package-ecosystem: cargo"), "{text}");
    assert!(
        !text.contains("vendor-copy") && !text.contains("copy") && !text.contains("npm"),
        "an entry was written for something that is not the repository: {text}"
    );
}
