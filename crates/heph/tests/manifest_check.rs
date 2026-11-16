//! `heph manifest check` as a person or a workflow meets it.

mod common;

use athenaeum::manifest::{CASES, EXAMPLE};
use common::{Scratch, heph_in, stderr, stdout};

fn case(name: &str) -> &'static str {
    CASES
        .iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("the pinned Athenaeum tag to carry the case {name}"))
        .toml
}

#[test]
fn a_valid_manifest_is_reported_valid_and_succeeds() {
    let scratch = Scratch::new("valid");
    scratch.write("m.toml", EXAMPLE);

    let output = heph_in(&scratch.0, &["manifest", "check", "m.toml"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "m.toml: valid\n");
    assert_eq!(stderr(&output), "");
}

#[test]
fn an_invalid_manifest_fails_and_says_where_and_why() {
    let scratch = Scratch::new("invalid");
    scratch.write("m.toml", case("zero-timeout"));

    let output = heph_in(&scratch.0, &["manifest", "check", "m.toml"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "", "a failure should not write to stdout");
    assert!(
        stderr(&output).contains("m.toml: $.gates.regression.timeout_seconds: "),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn without_a_path_it_checks_the_manifest_where_a_repository_keeps_it() {
    let scratch = Scratch::new("default");
    scratch.write(".athenaeum/manifest.toml", EXAMPLE);

    let output = heph_in(&scratch.0, &["manifest", "check"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), ".athenaeum/manifest.toml: valid\n");
}

#[test]
fn a_missing_manifest_fails_and_names_the_file() {
    let scratch = Scratch::new("missing");

    let output = heph_in(&scratch.0, &["manifest", "check", "absent.toml"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("absent.toml: cannot be read: "),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_file_that_is_not_toml_fails_and_says_so() {
    let scratch = Scratch::new("not-toml");
    scratch.write("m.toml", "schema_version = [unterminated");

    let output = heph_in(&scratch.0, &["manifest", "check", "m.toml"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("m.toml: is not TOML: "),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_command_line_that_cannot_be_understood_is_a_usage_error() {
    let scratch = Scratch::new("usage");

    for args in [
        &["manifest"][..],
        &["manifest", "validate"],
        &["manifest", "check", "one.toml", "two.toml"],
    ] {
        let output = heph_in(&scratch.0, args);

        assert_eq!(output.status.code(), Some(2), "for {args:?}");
        assert_eq!(stdout(&output), "", "for {args:?}");
        assert!(stderr(&output).contains("Usage: heph "), "for {args:?}");
    }
}
