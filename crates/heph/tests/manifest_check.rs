//! `heph manifest check` as a person or a workflow meets it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use athenaeum::manifest::{CASES, EXAMPLE};

/// A directory of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let directory = std::env::temp_dir().join(format!("heph-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a scratch directory");
        Self(directory)
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let file = self.0.join(relative);
        std::fs::create_dir_all(file.parent().expect("a parent directory"))
            .expect("the file's directory");
        std::fs::write(&file, text).expect("the file to be written");
        file
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn heph_in(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_heph"))
        .current_dir(directory)
        .args(args)
        .output()
        .expect("the heph binary to run")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout to be UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr to be UTF-8")
}

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
