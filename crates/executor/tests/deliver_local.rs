//! Making the commit locally, before anything is pushed: one commit of what the agent did,
//! on the branch the change will be pushed to, and the same commit amended when the
//! agent is asked to fix what a reader found. Nothing here pushes, and no token is held.

mod common;

use std::path::PathBuf;

use common::{Scratch, git, manifest, source, workload};
use contracts::v1alpha1::Workload;
use executor::deliver::{self, MESSAGE_FILE};
use serde_json::json;

struct Tree {
    scratch: Scratch,
    workspace: PathBuf,
    workload: Workload,
}

/// A workspace that is a clone's stand-in: the base commit on `main`, and the agent's change
/// in the working tree, with the message it wrote.
fn tree(name: &str, edit: impl FnOnce(&mut serde_json::Value)) -> Tree {
    let scratch = Scratch::new(name);
    let (workspace, base) = source(&scratch, &manifest(&[]));
    // What a clone excludes, so the agent's own files are never part of its change.
    std::fs::write(
        workspace.join(".git/info/exclude"),
        ".heph-commit-message\n.heph-description.md\n.heph-resolution\n",
    )
    .expect("exclude");
    // The executor's git runs with the maintainer's identity from its environment; here the
    // repository itself says who commits, and that nothing is signed.
    for (key, value) in [
        ("user.name", "test"),
        ("user.email", "test@example.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        git(&workspace, &["config", key, value]);
    }
    let mut value = workload(&base);
    edit(&mut value);
    let workload: Workload = serde_json::from_value(value).expect("a workload");
    scratch.write("source/web/change.txt", "the change\n");
    scratch.write(
        "source/.heph-commit-message",
        "feat: make the reset spec wait\n\nIt waits for hydration.\n",
    );
    Tree {
        scratch,
        workspace,
        workload,
    }
}

fn commits_past_main(tree: &Tree) -> Vec<String> {
    git(&tree.workspace, &["log", "--format=%s", "main..HEAD"])
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_agents_work_is_one_commit_on_the_branch_it_will_be_pushed_to() {
    let tree = tree("local-commit", |_| {});

    let committed = deliver::commit_locally(&tree.workload, &tree.workspace).expect("a commit");

    assert_eq!(
        committed.branch,
        "heph/12-make-the-reset-spec-wait-for-hydration"
    );
    assert_eq!(
        git(&tree.workspace, &["branch", "--show-current"]),
        committed.branch
    );
    assert_eq!(commits_past_main(&tree), ["feat: make the reset spec wait"]);
    assert_eq!(
        git(&tree.workspace, &["show", "HEAD:web/change.txt"]),
        "the change"
    );
    let files = git(
        &tree.workspace,
        &["show", "--name-only", "--format=", "HEAD"],
    );
    assert!(
        !files.contains(MESSAGE_FILE),
        "the message is not part of the change: {files}"
    );
    assert_eq!(git(&tree.workspace, &["status", "--porcelain"]), "");
}

#[test]
fn nothing_changed_is_nothing_to_commit_and_says_so() {
    let tree = tree("local-nothing", |_| {});
    std::fs::remove_file(tree.workspace.join("web/change.txt")).expect("to remove");

    let said = deliver::commit_locally(&tree.workload, &tree.workspace);

    let Err(reason) = said else {
        panic!("committed nothing");
    };
    assert!(reason.contains("changed nothing"), "{reason}");
    assert_eq!(commits_past_main(&tree).len(), 0);
}

#[test]
fn a_missing_or_unconventional_message_is_refused_before_anything_is_committed() {
    let tree = tree("local-message", |_| {});
    std::fs::remove_file(tree.workspace.join(MESSAGE_FILE)).expect("to remove");
    let Err(reason) = deliver::commit_locally(&tree.workload, &tree.workspace) else {
        panic!("committed without a message");
    };
    assert!(reason.contains("no commit message"), "{reason}");

    tree.scratch
        .write("source/.heph-commit-message", "made the change\n");
    let Err(reason) = deliver::commit_locally(&tree.workload, &tree.workspace) else {
        panic!("committed with a message that is not conventional");
    };
    assert!(reason.contains("conventional"), "{reason}");
    assert_eq!(commits_past_main(&tree).len(), 0);
}

#[test]
fn a_revisions_work_goes_on_the_pull_requests_own_branch() {
    let tree = tree("local-revision", |value| {
        value["revision"] = json!({
            "pullRequestNumber": "52",
            "pullRequestUrl": "https://github.com/erinversfeldcodes/gazebolang/pull/52",
            "branch": "heph/12-an-earlier-title",
            "attempt": 1,
            "failures": [{"name": "test", "result": "failure", "excerpt": "x"}]
        });
    });

    let committed = deliver::commit_locally(&tree.workload, &tree.workspace).expect("a commit");

    assert_eq!(committed.branch, "heph/12-an-earlier-title");
}

#[test]
fn amending_rewrites_the_one_commit_with_what_the_agent_changed_and_said() {
    let tree = tree("local-amend", |_| {});
    let committed = deliver::commit_locally(&tree.workload, &tree.workspace).expect("a commit");

    tree.scratch
        .write("source/web/change.txt", "the better change\n");
    tree.scratch.write("source/web/more.txt", "and more\n");
    tree.scratch.write(
        "source/.heph-commit-message",
        "fix: make the reset spec wait properly\n",
    );
    deliver::amend_locally(&committed, &tree.workspace).expect("an amend");

    assert_eq!(
        commits_past_main(&tree),
        ["fix: make the reset spec wait properly"],
        "still one commit, saying what it now is"
    );
    assert_eq!(
        git(&tree.workspace, &["show", "HEAD:web/change.txt"]),
        "the better change"
    );
    assert_eq!(
        git(&tree.workspace, &["show", "HEAD:web/more.txt"]),
        "and more"
    );
    assert_eq!(git(&tree.workspace, &["status", "--porcelain"]), "");
}

#[test]
fn amending_when_the_agent_changed_nothing_keeps_the_commit_as_it_was() {
    let tree = tree("local-amend-same", |_| {});
    let committed = deliver::commit_locally(&tree.workload, &tree.workspace).expect("a commit");

    deliver::amend_locally(&committed, &tree.workspace).expect("an amend");

    assert_eq!(commits_past_main(&tree).len(), 1);
    assert_eq!(
        git(&tree.workspace, &["show", "HEAD:web/change.txt"]),
        "the change"
    );
}
