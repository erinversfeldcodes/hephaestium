//! `heph gate run` with a gate that declares what it needs.
//!
//! A gate that needs something that is absent has not failed: it could not be
//! run, and a caller must be able to tell those apart. Each test states what
//! the gate would have done if it had run, so that "it was not run" is a thing
//! the test can see and not an inference.

mod common;

use common::{Scratch, conforms, heph_in, heph_with_env, stderr, stdout};
use serde_json::Value;

/// A manifest with one component, which the schema requires, and one gate
/// that creates `ran` when it runs and declares `requires`.
fn manifest(requires: &str, more: &str) -> String {
    format!(
        "schema_version = 0\n\n[components.app]\nroot = \".\"\npaths = [\"**\"]\n\n\
         [gates.needy]\nrun = [\"sh\", \"-c\", \"touch ran\"]\ntimeout_seconds = 30\n{more}\n\n\
         [gates.needy.requires]\n{requires}\n"
    )
}

fn scratch_with(name: &str, requires: &str, more: &str) -> Scratch {
    let scratch = Scratch::new(name);
    scratch.write(".athenaeum/manifest.toml", &manifest(requires, more));
    scratch
}

#[test]
fn a_gate_with_a_missing_variable_is_not_run_and_names_it() {
    let scratch = scratch_with("req-missing-one", "HEPH_TEST_NEEDS_DB = \"env\"", "");

    let output = heph_in(&scratch.0, &["gate", "run", "needy"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "",
        "a gate that could not be run has no result to print"
    );
    assert!(
        stderr(&output).contains("HEPH_TEST_NEEDS_DB"),
        "stderr should name the missing variable: {}",
        stderr(&output)
    );
    assert!(
        !scratch.exists("ran"),
        "the gate's command ran although a prerequisite was missing"
    );
}

#[test]
fn every_missing_variable_is_named_by_kind() {
    let scratch = scratch_with(
        "req-missing-several",
        "HEPH_TEST_NEEDS_DB = \"env\"\nHEPH_TEST_NEEDS_KEY = \"secret\"\nHEPH_TEST_NEEDS_URL = \"env\"",
        "",
    );

    let output = heph_in(&scratch.0, &["gate", "run", "needy"]);
    let message = stderr(&output);

    assert_eq!(output.status.code(), Some(3), "stderr: {message}");
    for name in [
        "HEPH_TEST_NEEDS_DB",
        "HEPH_TEST_NEEDS_KEY",
        "HEPH_TEST_NEEDS_URL",
    ] {
        assert!(message.contains(name), "{name} is not named: {message}");
    }
    assert!(
        message.contains("secret"),
        "a secret should be called one: {message}"
    );
    assert!(!scratch.exists("ran"));
}

#[test]
fn an_empty_value_counts_as_missing() {
    let scratch = scratch_with("req-empty", "HEPH_TEST_NEEDS_DB = \"env\"", "");

    let output = heph_with_env(
        &scratch.0,
        &["gate", "run", "needy"],
        &[("HEPH_TEST_NEEDS_DB", "")],
    );

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert!(!scratch.exists("ran"));
}

#[test]
fn the_value_of_a_secret_is_never_printed() {
    let scratch = scratch_with(
        "req-secret-value",
        "HEPH_TEST_NEEDS_KEY = \"secret\"\nHEPH_TEST_NEEDS_DB = \"env\"",
        "",
    );
    let marker = "s3cr3t-marker-9f2c1d";

    // The secret is present, and another prerequisite is not, so the tool
    // reports on stderr having seen the secret's name and its value.
    let output = heph_with_env(
        &scratch.0,
        &["gate", "run", "needy"],
        &[("HEPH_TEST_NEEDS_KEY", marker)],
    );

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert!(
        !stdout(&output).contains(marker) && !stderr(&output).contains(marker),
        "a secret's value was printed:\nstdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
}

#[test]
fn a_gate_whose_prerequisites_are_present_runs_as_before() {
    let scratch = scratch_with(
        "req-present",
        "HEPH_TEST_NEEDS_DB = \"env\"\nHEPH_TEST_NEEDS_KEY = \"secret\"",
        "",
    );

    let output = heph_with_env(
        &scratch.0,
        &["gate", "run", "needy"],
        &[
            ("HEPH_TEST_NEEDS_DB", "postgres://example"),
            ("HEPH_TEST_NEEDS_KEY", "value"),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let result: Value = serde_json::from_str(&stdout(&output)).expect("one line of JSON");
    conforms(&result);
    assert_eq!(result["outcome"], "passed");
    assert!(scratch.exists("ran"));
}

#[test]
fn a_gate_that_does_not_apply_to_the_change_is_skipped_without_its_prerequisites() {
    let scratch = scratch_with(
        "req-skipped",
        "HEPH_TEST_NEEDS_KEY = \"secret\"",
        "when_paths = [\"docs/**\"]",
    );

    let output = heph_in(
        &scratch.0,
        &["gate", "run", "needy", "--changed", "src/main.rs"],
    );

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let result: Value = serde_json::from_str(&stdout(&output)).expect("one line of JSON");
    conforms(&result);
    assert_eq!(
        result["outcome"], "skipped",
        "a gate that is not asked to run needs nothing"
    );
    assert!(!scratch.exists("ran"));
}

#[test]
fn the_manifest_check_accepts_requirements() {
    let scratch = scratch_with(
        "req-check",
        "HEPH_TEST_NEEDS_DB = \"env\"\nHEPH_TEST_NEEDS_KEY = \"secret\"",
        "",
    );

    let output = heph_in(&scratch.0, &["manifest", "check"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn the_manifest_check_accepts_a_gate_that_names_its_ci_check() {
    let scratch = scratch_with(
        "req-ci-check",
        "HEPH_TEST_NEEDS_DB = \"env\"",
        "ci_check = \"test\"",
    );

    let output = heph_in(&scratch.0, &["manifest", "check"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn heph_never_leaves_a_gate_to_a_check_itself_a_missing_variable_still_stops_it() {
    // Leaving a gate to CI is a choice for whoever opens a pull request that CI will
    // judge. `heph gate run` is the pipeline's own gate, and a pipeline that went green
    // because it deferred to itself would have checked nothing (ADR 0014).
    let scratch = scratch_with(
        "req-ci-check-run",
        "HEPH_TEST_NEEDS_DB = \"env\"",
        "ci_check = \"test\"",
    );

    let output = heph_in(&scratch.0, &["gate", "run", "needy"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert!(!scratch.exists("ran"));
}
