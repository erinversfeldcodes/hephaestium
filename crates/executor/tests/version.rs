//! The executor says which version it is, as `heph` does, so that a release can prove each
//! binary it ships reports the version being released.

use std::process::Command;

fn executor(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_executor"))
        .args(args)
        .output()
        .expect("the executor runs")
}

#[test]
fn it_reports_its_version_and_exits_zero() {
    let out = executor(&["--version"]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let mut words = text.split_whitespace();
    assert_eq!(words.next(), Some("executor"), "{text}");
    assert!(
        words.next().is_some_and(|v| !v.is_empty()),
        "a version follows: {text}"
    );
    assert_eq!(words.next(), None, "nothing else: {text}");
}

#[test]
fn the_short_flag_says_the_same() {
    let long = executor(&["--version"]).stdout;

    assert!(!long.is_empty(), "the long flag says something");
    assert_eq!(executor(&["-V"]).stdout, long);
}

#[test]
fn a_version_flag_beside_other_arguments_is_still_a_usage_error() {
    let out = executor(&["--version", "run"]);

    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn an_unknown_command_is_still_a_usage_error() {
    assert_eq!(executor(&["--nonsense"]).status.code(), Some(2));
}
