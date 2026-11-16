//! `heph lint` as a person or a workflow meets it.
//!
//! Lint commands here are small shell commands that leave a marker file where
//! they ran, so each test can say which commands ran, and from where. Every
//! result is held to the pinned Athenaeum gate result schema.

mod common;

use std::process::Output;
use std::time::{Duration, Instant};

use common::{Scratch, conforms, heph_in, heph_with_env, stderr, stdout};
use serde_json::Value;

/// A command that leaves a marker file in the directory it runs in.
fn touches(marker: &str) -> String {
    format!("[\"sh\", \"-c\", \"touch {marker}\"]")
}

/// A manifest with these components, written out in full.
fn manifest(extra: &str, components: &str) -> String {
    format!("schema_version = 0\n\n{extra}\n{components}\n")
}

fn component(name: &str, root: &str, paths: &str, lint: &str) -> String {
    format!("[components.{name}]\nroot = {root:?}\npaths = {paths}\n{lint}\n")
}

fn results_of(output: &Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .map(|line| {
            let result: Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("{line:?} should be JSON: {error}"));
            conforms(&result);
            result
        })
        .collect()
}

fn names_of(results: &[Value]) -> Vec<&str> {
    results
        .iter()
        .map(|result| result["gate"].as_str().expect("a gate name"))
        .collect()
}

/// Two components, each in its own directory, each leaving its own marker.
fn two_components() -> String {
    manifest(
        "",
        &format!(
            "{}{}",
            component(
                "api",
                "api",
                "[\"api/**\"]",
                &format!("lint = [{}]", touches("api-linted"))
            ),
            component(
                "web",
                "web",
                "[\"web/**\"]",
                &format!("lint = [{}]", touches("web-linted"))
            ),
        ),
    )
}

fn repository(name: &str, manifest: &str) -> Scratch {
    let scratch = Scratch::new(name);
    scratch.write(".athenaeum/manifest.toml", manifest);
    std::fs::create_dir_all(scratch.0.join("api")).expect("api directory");
    std::fs::create_dir_all(scratch.0.join("web")).expect("web directory");
    scratch
}

#[test]
fn without_a_list_of_changes_every_components_lint_commands_run() {
    let scratch = repository("all", &two_components());

    let output = heph_in(&scratch.0, &["lint"]);
    let results = results_of(&output);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(names_of(&results), ["lint:api:1", "lint:web:1"]);
    assert!(results.iter().all(|result| result["outcome"] == "passed"));
    assert!(scratch.exists("api/api-linted"));
    assert!(scratch.exists("web/web-linted"));
}

#[test]
fn a_command_runs_from_its_components_root_and_not_the_repository_root() {
    let scratch = repository("cwd", &two_components());

    heph_in(&scratch.0, &["lint"]);

    assert!(scratch.exists("api/api-linted"), "ran in the api root");
    assert!(
        !scratch.exists("api-linted"),
        "it must not run in the repository root"
    );
}

#[test]
fn only_the_components_a_change_touches_are_linted() {
    let scratch = repository("touched", &two_components());

    let output = heph_in(&scratch.0, &["lint", "--changed", "api/src/main.rs"]);
    let results = results_of(&output);

    assert_eq!(names_of(&results), ["lint:api:1"]);
    assert!(scratch.exists("api/api-linted"));
    assert!(!scratch.exists("web/web-linted"), "web was not touched");
}

#[test]
fn a_change_that_touches_no_component_runs_nothing_and_succeeds() {
    let scratch = repository("untouched", &two_components());

    let output = heph_in(&scratch.0, &["lint", "--changed", "README.md"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "", "there is nothing to report");
    assert!(!scratch.exists("api/api-linted"));
    assert!(!scratch.exists("web/web-linted"));
}

#[test]
fn a_component_with_several_commands_runs_each_numbered_from_one() {
    let manifest = manifest(
        "",
        &component(
            "api",
            "api",
            "[\"api/**\"]",
            &format!(
                "lint = [{}, {}, {}]",
                touches("first"),
                touches("second"),
                touches("third")
            ),
        ),
    );
    let scratch = repository("several", &manifest);

    let results = results_of(&heph_in(&scratch.0, &["lint"]));

    assert_eq!(
        names_of(&results),
        ["lint:api:1", "lint:api:2", "lint:api:3"]
    );
}

#[test]
fn a_component_with_no_lint_commands_has_nothing_to_run() {
    let manifest = manifest(
        "",
        &format!(
            "{}{}",
            component("docs", ".", "[\"docs/**\"]", ""),
            component(
                "api",
                "api",
                "[\"api/**\"]",
                &format!("lint = [{}]", touches("api-linted"))
            ),
        ),
    );
    let scratch = repository("no-lint", &manifest);

    let results = results_of(&heph_in(&scratch.0, &["lint"]));

    assert_eq!(names_of(&results), ["lint:api:1"]);
}

#[test]
fn every_command_runs_even_when_an_earlier_one_fails_and_the_run_fails() {
    let manifest = manifest(
        "",
        &format!(
            "{}{}",
            component(
                "api",
                "api",
                "[\"api/**\"]",
                &format!(
                    "lint = [[\"sh\", \"-c\", \"exit 7\"], {}]",
                    touches("after")
                )
            ),
            component(
                "web",
                "web",
                "[\"web/**\"]",
                &format!("lint = [{}]", touches("web-linted"))
            ),
        ),
    );
    let scratch = repository("failing", &manifest);

    let output = heph_in(&scratch.0, &["lint"]);
    let results = results_of(&output);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        names_of(&results),
        ["lint:api:1", "lint:api:2", "lint:web:1"]
    );
    assert_eq!(results[0]["outcome"], "failed");
    assert_eq!(results[0]["exit_code"], 7);
    assert_eq!(results[1]["outcome"], "passed");
    assert!(scratch.exists("api/after"), "a later command still ran");
    assert!(
        scratch.exists("web/web-linted"),
        "a later component still ran"
    );
}

#[test]
fn the_toolchain_wrapper_is_put_in_front_of_every_command() {
    let manifest = manifest(
        "[toolchain]\nwrapper = [\"env\", \"WRAPPED=yes\"]\ninside_env = \"HEPH_TEST_INSIDE\"\n",
        &component(
            "api",
            "api",
            "[\"api/**\"]",
            "lint = [[\"sh\", \"-c\", \"test \\\"$WRAPPED\\\" = yes\"]]",
        ),
    );
    let scratch = repository("wrapper", &manifest);

    let wrapped = results_of(&heph_in(&scratch.0, &["lint"]));
    let unwrapped = results_of(&heph_with_env(
        &scratch.0,
        &["lint"],
        &[("HEPH_TEST_INSIDE", "1")],
    ));

    assert_eq!(wrapped[0]["outcome"], "passed", "{wrapped:?}");
    assert_eq!(wrapped[0]["command"][0], "env");
    assert_eq!(
        unwrapped[0]["outcome"], "failed",
        "inside the toolchain the wrapper is left off, so WRAPPED is unset"
    );
}

#[test]
fn a_command_is_stopped_at_the_timeout_it_is_given() {
    let manifest = manifest(
        "",
        &component("api", "api", "[\"api/**\"]", "lint = [[\"sleep\", \"30\"]]"),
    );
    let scratch = repository("timeout", &manifest);

    let started = Instant::now();
    let output = heph_in(&scratch.0, &["lint", "--timeout-seconds", "1"]);
    let results = results_of(&output);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(results[0]["outcome"], "timed_out");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_component_whose_root_is_not_a_directory_could_not_be_linted() {
    let manifest = manifest(
        "",
        &component(
            "api",
            "no-such-directory",
            "[\"api/**\"]",
            &format!("lint = [{}]", touches("api-linted")),
        ),
    );
    let scratch = repository("bad-root", &manifest);

    let output = heph_in(&scratch.0, &["lint"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("no-such-directory"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn an_invalid_manifest_could_not_be_linted_and_the_error_says_where() {
    let scratch = repository(
        "invalid",
        "schema_version = 0\n\n[components.api]\nroot = \"/absolute\"\npaths = [\"api/**\"]\n",
    );

    let output = heph_in(&scratch.0, &["lint"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains("$.components.api.root"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_missing_manifest_could_not_be_linted_and_the_error_names_the_file() {
    let scratch = Scratch::new("no-manifest");

    let output = heph_in(&scratch.0, &["lint"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(stderr(&output).contains(".athenaeum/manifest.toml"));
}

#[test]
fn a_command_line_that_cannot_be_understood_is_a_usage_error() {
    let scratch = Scratch::new("usage");

    for args in [
        &["lint", "extra"][..],
        &["lint", "--changed"],
        &["lint", "--timeout-seconds"],
        &["lint", "--timeout-seconds", "soon"],
        &["lint", "--timeout-seconds", "0"],
        &["lint", "--nonsense"],
    ] {
        let output = heph_in(&scratch.0, args);

        assert_eq!(output.status.code(), Some(2), "for {args:?}");
        assert_eq!(stdout(&output), "", "for {args:?}");
        assert!(stderr(&output).contains("Usage: heph "), "for {args:?}");
    }
}
