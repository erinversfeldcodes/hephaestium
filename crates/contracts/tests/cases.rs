//! Every case under `cases/runtime/`, read through the generated types.
//!
//! The cases are the contract's conformance suite: a reader in any language is
//! graded by the same files. A valid case must be accepted. An invalid case must
//! be rejected, and for the reason its `.violates` sidecar states, because a
//! case rejected for some other reason proves nothing about the rule it was
//! written for.

use std::fs;
use std::path::{Path, PathBuf};

use contracts::v1alpha1::{Adapter, Event, LaunchPlan, RunRecord, Workload};

type Reader = fn(&str) -> Result<(), String>;

fn reader<T: serde::de::DeserializeOwned>() -> Reader {
    |text| {
        serde_json::from_str::<T>(text)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// The message each case directory is for. A directory with no entry here is a
/// failure, so a new contract cannot be added and left ungraded.
fn readers() -> Vec<(&'static str, Reader)> {
    vec![
        ("Workload", reader::<Workload>()),
        ("Event", reader::<Event>()),
        ("RunRecord", reader::<RunRecord>()),
        ("Adapter", reader::<Adapter>()),
        ("LaunchPlan", reader::<LaunchPlan>()),
    ]
}

fn cases_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cases/runtime")
}

fn files(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(directory)
        .unwrap_or_else(|e| panic!("{}: {e}", directory.display()))
        .map(|entry| entry.expect("an entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == extension))
        .collect();
    found.sort();
    found
}

#[test]
fn every_case_directory_has_a_reader() {
    let known: Vec<&str> = readers().iter().map(|(name, _)| *name).collect();
    for directory in fs::read_dir(cases_root()).expect("cases").flatten() {
        let name = directory.file_name().to_string_lossy().into_owned();
        assert!(
            known.contains(&name.as_str()),
            "cases/runtime/{name} has no reader in this test"
        );
    }
}

#[test]
fn every_valid_case_is_accepted() {
    for (name, read) in readers() {
        let directory = cases_root().join(name).join("valid");
        for case in files(&directory, "json") {
            let text = fs::read_to_string(&case).expect("a case");
            if let Err(error) = read(&text) {
                panic!("{}: a valid case was rejected: {error}", case.display());
            }
        }
    }
}

/// How this reader words each kind of violation, matched against its error.
/// Each reader brings its own table: this one reports a position for a bad value
/// and not the value, so only an unknown field or enum name is also checked for
/// the name the case states.
fn wording(kind: &str) -> &'static [&'static str] {
    match kind {
        "unknown-field" => &["unknown field"],
        "bad-enum" => &["unknown variant", "unknown enum", "invalid value"],
        "bad-duration" => &["invalid", "duration"],
        "bad-timestamp" => &["premature end of input", "invalid", "timestamp"],
        "bad-number" => &["invalid", "out of range", "number", "integer"],
        "wrong-type" => &["invalid type", "expected"],
        "multiple-oneof" => &["multiple", "oneof", "duplicate"],
        other => panic!("a case states a kind this test has no wording for: {other}"),
    }
}

#[test]
fn every_invalid_case_is_rejected_for_the_reason_it_states() {
    let mut failures = Vec::new();
    for (name, read) in readers() {
        let directory = cases_root().join(name).join("invalid");
        for case in files(&directory, "json") {
            let sidecar = case.with_extension("violates");
            let statement = fs::read_to_string(&sidecar)
                .unwrap_or_else(|_| panic!("{} has no .violates sidecar", case.display()));
            let (kind, what) = statement
                .trim()
                .split_once(' ')
                .unwrap_or_else(|| panic!("{}: not '<kind> <what>'", sidecar.display()));
            let text = fs::read_to_string(&case).expect("a case");
            let Err(error) = read(&text) else {
                failures.push(format!(
                    "{}: was accepted, but must be rejected as {kind}",
                    case.display()
                ));
                continue;
            };
            let lower = error.to_lowercase();
            if !wording(kind).iter().any(|w| lower.contains(w)) {
                failures.push(format!(
                    "{}: rejected for the wrong reason\n  stated: {kind} {what}\n  reported: {error}",
                    case.display()
                ));
            } else if matches!(kind, "unknown-field" | "bad-enum")
                && !lower.contains(&what.to_lowercase())
            {
                failures.push(format!(
                    "{}: rejected as {kind}, but the report does not mention '{what}'\n  reported: {error}",
                    case.display()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
