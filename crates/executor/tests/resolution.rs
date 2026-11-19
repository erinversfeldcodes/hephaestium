//! Whether a pull request closes its ticket. A ticket is closed only when every item
//! that affects it is resolved, so the pull request says `Closes #N` only when the
//! agent says it resolves the whole ticket, and `Refs #N` otherwise.

mod common;

use common::{BRANCH, IDENTITY, agent_script, finished, git, manifest, rig, run};
use contracts::v1alpha1::finished::Outcome;

/// A delivered pull request whose agent also wrote this to `.heph-resolution` (or wrote
/// nothing, for `None`), and the body it was opened with.
fn delivered(name: &str, resolution: Option<&str>) -> (common::Ran, std::path::PathBuf, String) {
    let extra = resolution.map_or(String::new(), |text| {
        format!("printf '{}' > .heph-resolution;", text.replace('\n', "\\n"))
    });
    let (ran, source, _) = run(
        rig(name, &manifest(&[])),
        &agent_script(&extra),
        true,
        0,
        &IDENTITY,
    );
    let body = std::fs::read_to_string(ran.scratch.path("seen/gh-body.txt")).unwrap_or_default();
    (ran, source, body)
}

fn last_line(body: &str) -> &str {
    body.trim_end().lines().last().unwrap_or_default()
}

#[test]
fn a_complete_resolution_closes_the_ticket() {
    let (ran, _, body) = delivered("resolution-complete", Some("complete\n"));

    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    assert_eq!(last_line(&body), "Closes #12", "{body}");
    assert!(!body.contains("Not resolved"), "{body}");
    assert!(!body.contains("Refs #12"), "{body}");
}

#[test]
fn the_word_is_read_without_regard_to_case_or_whitespace() {
    for text in ["Complete", "  complete  \n\n", "COMPLETE\n"] {
        let (_, _, body) = delivered("resolution-case", Some(text));

        assert_eq!(last_line(&body), "Closes #12", "{text:?}: {body}");
    }
}

#[test]
fn a_partial_resolution_refers_to_the_ticket_and_says_what_remains() {
    let (ran, _, body) = delivered(
        "resolution-partial",
        Some("partial: the failure rates are not measured\n"),
    );

    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    assert_eq!(last_line(&body), "Refs #12", "{body}");
    assert!(
        body.contains("Not resolved: the failure rates are not measured"),
        "{body}"
    );
    assert!(!body.contains("Closes"), "{body}");
}

#[test]
fn a_partial_resolution_that_says_nothing_of_what_remains_says_so() {
    let (_, _, body) = delivered("resolution-partial-bare", Some("partial\n"));

    assert_eq!(last_line(&body), "Refs #12", "{body}");
    assert!(
        body.contains("Not resolved: the agent did not say what remains"),
        "{body}"
    );
}

#[test]
fn without_the_agent_saying_so_the_ticket_is_only_referred_to() {
    let cases: [(&str, Option<&str>); 6] = [
        ("absent", None),
        ("empty", Some("")),
        ("blank", Some("  \n\n")),
        ("another word", Some("done\n")),
        ("a longer word", Some("completely\n")),
        ("more than the word", Some("complete\nexcept the docs\n")),
    ];

    for (name, text) in cases {
        let (ran, _, body) = delivered(&format!("resolution-unsaid-{}", name.len()), text);

        assert_eq!(
            finished(&ran.events()).0,
            Outcome::Completed,
            "{name}: {}",
            ran.stderr()
        );
        assert_eq!(last_line(&body), "Refs #12", "{name}: {body}");
        assert!(!body.contains("Closes"), "{name}: {body}");
    }
}

#[test]
fn the_resolution_file_is_never_part_of_the_change() {
    let (ran, source, _) = delivered("resolution-not-committed", Some("complete\n"));

    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    let files = git(&source, &["ls-tree", "-r", "--name-only", BRANCH]);
    assert!(
        !files.lines().any(|file| file == ".heph-resolution"),
        "the file was committed:\n{files}"
    );
}

#[test]
fn a_workload_that_changed_nothing_still_fails_whatever_its_resolution_says() {
    let (ran, _, _) = run(
        rig("resolution-nothing", &manifest(&[])),
        // A message and a resolution, but no change to the tree.
        "printf 'fix: nothing\\n' > .heph-commit-message; printf 'complete\\n' > .heph-resolution;",
        true,
        0,
        &IDENTITY,
    );

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed, "{}", ran.stderr());
    assert!(reason.contains("changed nothing"), "{reason}");
}
