//! The command line as a person or a workflow meets it: what is printed, on
//! which stream, and with what exit code.

use std::process::{Command, Output};

/// What a build reports when no version was compiled in.
const DEVELOPMENT_VERSION: &str = "0.0.0-dev";

fn heph(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_heph"))
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

/// The version this test binary was built with. The tests and the binary are
/// compiled in the same environment, so they agree on it: a release build
/// names the release, and any other build names a development version.
fn expected_version() -> &'static str {
    match option_env!("HEPHAESTIUM_VERSION") {
        Some(version) if !version.is_empty() => version,
        _ => DEVELOPMENT_VERSION,
    }
}

#[test]
fn version_is_one_line_naming_the_tool_and_its_version() {
    for flag in ["--version", "-V"] {
        let output = heph(&[flag]);

        assert!(output.status.success(), "{flag} should succeed");
        assert_eq!(stdout(&output), format!("heph {}\n", expected_version()));
        assert_eq!(stderr(&output), "", "{flag} should not write to stderr");
    }
}

#[test]
fn help_goes_to_stdout_and_succeeds() {
    for flag in ["--help", "-h"] {
        let output = heph(&[flag]);

        assert!(output.status.success(), "{flag} should succeed");
        assert!(stdout(&output).starts_with("Usage: heph "));
        assert_eq!(stderr(&output), "", "{flag} should not write to stderr");
    }
}

#[test]
fn no_arguments_is_a_usage_error() {
    let output = heph(&[]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "", "an error should not write to stdout");
    assert!(stderr(&output).contains("Usage: heph "));
}

#[test]
fn an_unknown_argument_is_named_in_the_error() {
    let output = heph(&["--nonsense"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "", "an error should not write to stdout");
    assert!(stderr(&output).contains("--nonsense"));
    assert!(stderr(&output).contains("Usage: heph "));
}
