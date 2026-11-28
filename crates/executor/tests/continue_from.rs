//! A run that is given the change an earlier attempt left applies it to the fresh clone as a
//! patch, and its record says which attempt it continued; one that is not given it, or whose
//! patch does not apply, starts from the base and its record says it continued nothing.

mod common;

use std::path::PathBuf;

use common::{Scratch, manifest, run_with, source, transcript, workload};
use contracts::v1alpha1::event::Kind;

const NEW_FILE: &str = "diff --git a/kept.txt b/kept.txt\nnew file mode 100644\nindex 0000000..e69de29\n--- /dev/null\n+++ b/kept.txt\n@@ -0,0 +1 @@\n+kept\n";
const STALE: &str = "diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-no such line\n+changed\n";

fn run_continuing(name: &str, patch: Option<&str>) -> common::Ran {
    let scratch = Scratch::new(name);
    let (_, commit) = source(&scratch, &manifest(&[]));
    let mut extra = Vec::new();
    let file;
    if let Some(patch) = patch {
        file = scratch.write("earlier.patch", patch);
        extra.extend([
            "--continue-patch".to_owned(),
            file.display().to_string(),
            "--continued-attempt".to_owned(),
            "earlier-workload".to_owned(),
        ]);
    }
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    run_with(scratch, &workload(&commit), Some(&transcript()), &extra)
}

fn workspace(ran: &common::Ran) -> PathBuf {
    let events = ran.events();
    let Some(Kind::WorkspaceReady(ready)) = events[1].kind.as_ref() else {
        panic!("no workspace_ready");
    };
    PathBuf::from(&ready.path)
}

#[test]
fn a_run_given_an_earlier_change_starts_from_it_and_its_record_names_the_attempt() {
    let ran = run_continuing("continues", Some(NEW_FILE));
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert!(workspace(&ran).join("kept.txt").is_file());
    assert_eq!(ran.record().continued_attempt, "earlier-workload");
}

#[test]
fn a_run_given_no_earlier_change_starts_from_the_base_and_its_record_continues_nothing() {
    let ran = run_continuing("fresh", None);
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert!(!workspace(&ran).join("kept.txt").exists());
    assert_eq!(ran.record().continued_attempt, "");
}

#[test]
fn a_change_that_does_not_apply_leaves_the_clone_at_the_base_and_says_so() {
    let ran = run_continuing("stale", Some(STALE));
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(ran.record().continued_attempt, "");
    assert!(ran.stderr().contains("does not apply"), "{}", ran.stderr());
}
