//! `heph gate run` as a person or a workflow meets it.
//!
//! Gates here are small shell commands, so each test states exactly what the
//! gate does and what the result must say about it. Every result is also held
//! to the pinned Athenaeum gate result schema, whatever else a test checks.

mod common;

use std::process::Output;
use std::time::{Duration, Instant};

use athenaeum::gate_result::SCHEMA;
use common::{Scratch, heph_in, heph_with_env, stderr, stdout};
use serde_json::Value;

/// A manifest with one component, which the schema requires, and these
/// gates. `extra` goes before the gates, so it can hold `[toolchain]`.
fn manifest(extra: &str, gates: &str) -> String {
    format!(
        "schema_version = 0\n\n{extra}\n[components.app]\nroot = \".\"\npaths = [\"**\"]\n\n{gates}\n"
    )
}

fn gate(name: &str, run: &str, timeout_seconds: u32, more: &str) -> String {
    format!("[gates.{name}]\nrun = {run}\ntimeout_seconds = {timeout_seconds}\n{more}\n")
}

fn shell(script: &str) -> String {
    format!("[\"sh\", \"-c\", {script:?}]")
}

fn result_of(output: &Output) -> Value {
    let text = stdout(output);
    assert_eq!(
        text.lines().count(),
        1,
        "stdout should be one line of JSON, but is: {text:?}\nstderr: {}",
        stderr(output)
    );
    let result: Value = serde_json::from_str(&text).expect("stdout to be JSON");
    conforms(&result);
    result
}

/// Hold a result to the gate result schema of the Athenaeum tag this tool
/// pins, so that a result the tool writes is one the contract accepts.
fn conforms(result: &Value) {
    let schema: Value = serde_json::from_str(SCHEMA).expect("the schema to be JSON");
    let validator = jsonschema::validator_for(&schema).expect("the schema to compile");
    let problems: Vec<String> = validator
        .iter_errors(result)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect();
    assert!(
        problems.is_empty(),
        "the result does not conform to the gate result schema:\n{}\nresult: {result}",
        problems.join("\n")
    );
}

#[test]
fn a_gate_that_exits_zero_passed() {
    let scratch = Scratch::new("passed");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest("", &gate("regression", "[\"true\"]", 30, "")),
    );

    let output = heph_in(&scratch.0, &["gate", "run", "regression"]);
    let result = result_of(&output);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(result["gate"], "regression");
    assert_eq!(result["outcome"], "passed");
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["command"], serde_json::json!(["true"]));
}

#[test]
fn a_gate_that_exits_non_zero_failed_with_its_exit_code() {
    let scratch = Scratch::new("failed");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest("", &gate("regression", &shell("exit 3"), 30, "")),
    );

    let output = heph_in(&scratch.0, &["gate", "run", "regression"]);
    let result = result_of(&output);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["exit_code"], 3);
}

#[test]
fn a_gate_killed_by_a_signal_failed_with_the_shell_convention_for_it() {
    let scratch = Scratch::new("signalled");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest("", &gate("regression", &shell("kill -TERM $$"), 30, "")),
    );

    let result = result_of(&heph_in(&scratch.0, &["gate", "run", "regression"]));

    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["exit_code"], 128 + 15, "128 plus the signal number");
}

#[test]
fn a_gate_that_runs_too_long_timed_out_and_has_no_exit_code() {
    let scratch = Scratch::new("timed-out");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest("", &gate("slow", "[\"sleep\", \"30\"]", 1, "")),
    );

    let started = Instant::now();
    let output = heph_in(&scratch.0, &["gate", "run", "slow"]);
    let waited = started.elapsed();
    let result = result_of(&output);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(result["outcome"], "timed_out");
    assert!(result.get("exit_code").is_none());
    assert!(
        waited < Duration::from_secs(10),
        "it should not have waited for the command: {waited:?}"
    );
    assert!(
        result["duration_ms"].as_u64().expect("a duration") >= 1000,
        "it should have been given its whole second: {result}"
    );
}

#[test]
fn a_timed_out_gate_leaves_nothing_it_started_running() {
    let scratch = Scratch::new("grandchild");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &gate(
                "slow",
                &shell("sleep 60 & echo $! > child.pid; wait"),
                1,
                "",
            ),
        ),
    );

    let result = result_of(&heph_in(&scratch.0, &["gate", "run", "slow"]));
    assert_eq!(result["outcome"], "timed_out");

    let pid = std::fs::read_to_string(scratch.0.join("child.pid"))
        .expect("the gate to have recorded what it started")
        .trim()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid])
            .output()
            .expect("kill to run")
            .status
            .success();
        if !alive {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "process {pid}, started by the gate, is still running"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_gate_whose_paths_do_not_match_the_change_is_skipped_and_not_run() {
    let scratch = Scratch::new("skipped");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &gate(
                "api-only",
                &shell("touch ran.marker"),
                30,
                "when_paths = [\"api/**\"]",
            ),
        ),
    );

    let output = heph_in(
        &scratch.0,
        &["gate", "run", "api-only", "--changed", "web/src/app.ts"],
    );
    let result = result_of(&output);

    assert_eq!(output.status.code(), Some(0), "skipped is not a failure");
    assert_eq!(result["outcome"], "skipped");
    assert!(
        result["reason"]
            .as_str()
            .expect("a reason")
            .contains("when_paths"),
        "{result}"
    );
    assert!(result.get("exit_code").is_none());
    assert_eq!(result["duration_ms"], 0);
    assert!(!scratch.exists("ran.marker"), "a skipped gate must not run");
}

#[test]
fn a_gate_whose_paths_match_a_changed_file_runs() {
    let scratch = Scratch::new("applies");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &gate(
                "api-only",
                &shell("touch ran.marker"),
                30,
                "when_paths = [\"api/**\"]",
            ),
        ),
    );

    let result = result_of(&heph_in(
        &scratch.0,
        &[
            "gate",
            "run",
            "api-only",
            "--changed",
            "web/src/app.ts",
            "--changed",
            "./api/src/main.rs",
        ],
    ));

    assert_eq!(result["outcome"], "passed");
    assert!(scratch.exists("ran.marker"));
}

#[test]
fn without_a_list_of_changes_every_gate_applies() {
    let scratch = Scratch::new("no-changes-given");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &gate(
                "api-only",
                &shell("touch ran.marker"),
                30,
                "when_paths = [\"api/**\"]",
            ),
        ),
    );

    let result = result_of(&heph_in(&scratch.0, &["gate", "run", "api-only"]));

    assert_eq!(result["outcome"], "passed");
    assert!(scratch.exists("ran.marker"));
}

#[test]
fn a_star_does_not_cross_a_directory_but_a_double_star_does() {
    let scratch = Scratch::new("globs");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &format!(
                "{}{}",
                gate("shallow", "[\"true\"]", 30, "when_paths = [\"docs/*.md\"]"),
                gate("deep", "[\"true\"]", 30, "when_paths = [\"docs/**/*.md\"]")
            ),
        ),
    );

    let outcome_of = |name: &str, changed: &str| {
        result_of(&heph_in(
            &scratch.0,
            &["gate", "run", name, "--changed", changed],
        ))["outcome"]
            .as_str()
            .expect("an outcome")
            .to_owned()
    };

    assert_eq!(outcome_of("shallow", "docs/a.md"), "passed");
    assert_eq!(outcome_of("shallow", "docs/deep/a.md"), "skipped");
    assert_eq!(outcome_of("deep", "docs/deep/a.md"), "passed");
}

#[test]
fn the_toolchain_wrapper_is_put_in_front_of_the_command() {
    let scratch = Scratch::new("wrapper");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "[toolchain]\nwrapper = [\"env\", \"WRAPPED=yes\"]\n",
            &gate("regression", &shell("test \"$WRAPPED\" = yes"), 30, ""),
        ),
    );

    let result = result_of(&heph_in(&scratch.0, &["gate", "run", "regression"]));

    assert_eq!(result["outcome"], "passed", "{result}");
    assert_eq!(
        result["command"],
        serde_json::json!(["env", "WRAPPED=yes", "sh", "-c", "test \"$WRAPPED\" = yes"]),
        "the result records what ran, wrapper included"
    );
}

#[test]
fn the_wrapper_is_not_applied_when_the_caller_is_already_inside_the_toolchain() {
    let scratch = Scratch::new("inside");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "[toolchain]\nwrapper = [\"false\"]\ninside_env = \"HEPH_TEST_INSIDE\"\n",
            &gate("regression", "[\"true\"]", 30, ""),
        ),
    );

    let outside = result_of(&heph_in(&scratch.0, &["gate", "run", "regression"]));
    let inside = result_of(&heph_with_env(
        &scratch.0,
        &["gate", "run", "regression"],
        &[("HEPH_TEST_INSIDE", "1")],
    ));

    assert_eq!(outside["outcome"], "failed", "the wrapper `false` ran");
    assert_eq!(inside["outcome"], "passed", "the wrapper was left off");
    assert_eq!(inside["command"], serde_json::json!(["true"]));
}

#[test]
fn a_program_that_does_not_exist_fails_the_gate_with_127() {
    let scratch = Scratch::new("missing-program");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &gate("regression", "[\"heph-test-no-such-program\"]", 30, ""),
        ),
    );

    let output = heph_in(&scratch.0, &["gate", "run", "regression"]);
    let result = result_of(&output);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["exit_code"], 127);
    assert!(
        stderr(&output).contains("heph-test-no-such-program"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_file_that_cannot_be_executed_fails_the_gate_with_126() {
    let scratch = Scratch::new("not-executable");
    scratch.write("not-a-program", "plain text");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest("", &gate("regression", "[\"./not-a-program\"]", 30, "")),
    );

    let result = result_of(&heph_in(&scratch.0, &["gate", "run", "regression"]));

    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["exit_code"], 126);
}

#[test]
fn what_the_gate_prints_goes_to_stderr_and_stdout_carries_only_the_result() {
    let scratch = Scratch::new("output");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &gate(
                "regression",
                &shell("echo noise-on-stdout; echo noise-on-stderr >&2"),
                30,
                "",
            ),
        ),
    );

    let output = heph_in(&scratch.0, &["gate", "run", "regression"]);
    let result = result_of(&output);

    assert_eq!(result["outcome"], "passed");
    assert!(!stdout(&output).contains("noise"));
    assert!(stderr(&output).contains("noise-on-stdout"));
    assert!(stderr(&output).contains("noise-on-stderr"));
}

#[test]
fn a_gate_that_reads_its_input_is_given_none() {
    let scratch = Scratch::new("stdin");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest("", &gate("regression", &shell("cat > /dev/null"), 30, "")),
    );

    let started = Instant::now();
    let result = result_of(&heph_in(&scratch.0, &["gate", "run", "regression"]));

    assert_eq!(result["outcome"], "passed");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_gate_that_reports_json_has_its_report_returned() {
    let scratch = Scratch::new("report");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &gate(
                "preview",
                &shell("echo '{\"journeys\": 6, \"failed\": 0}'"),
                30,
                "result = \"json\"",
            ),
        ),
    );

    let result = result_of(&heph_in(&scratch.0, &["gate", "run", "preview"]));

    assert_eq!(result["outcome"], "passed");
    assert_eq!(
        result["report"],
        serde_json::json!({"journeys": 6, "failed": 0})
    );
}

#[test]
fn a_passing_gate_whose_report_is_not_a_json_object_could_not_be_run() {
    let scratch = Scratch::new("bad-report");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &gate("preview", &shell("echo not json"), 30, "result = \"json\""),
        ),
    );

    let output = heph_in(&scratch.0, &["gate", "run", "preview"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(stdout(&output), "", "no result is better than a false one");
    assert!(
        stderr(&output).contains("JSON object"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_failing_gate_with_an_unusable_report_still_fails_without_one() {
    let scratch = Scratch::new("failed-bad-report");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &gate(
                "preview",
                &shell("echo crashed before reporting; exit 4"),
                30,
                "result = \"json\"",
            ),
        ),
    );

    let output = heph_in(&scratch.0, &["gate", "run", "preview"]);
    let result = result_of(&output);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["exit_code"], 4);
    assert!(result.get("report").is_none());
}

#[test]
fn a_gate_the_manifest_does_not_declare_could_not_be_run_and_the_error_lists_those_it_does() {
    let scratch = Scratch::new("unknown-gate");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest(
            "",
            &format!(
                "{}{}",
                gate("regression", "[\"true\"]", 30, ""),
                gate("preview", "[\"true\"]", 30, "")
            ),
        ),
    );

    let output = heph_in(&scratch.0, &["gate", "run", "regresion"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("regresion"), "{}", stderr(&output));
    assert!(stderr(&output).contains("preview"), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("regression"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_invalid_manifest_could_not_be_run_and_the_error_says_where() {
    let scratch = Scratch::new("invalid-manifest");
    scratch.write(
        ".athenaeum/manifest.toml",
        &manifest("", &gate("regression", "[\"true\"]", 0, "")),
    );

    let output = heph_in(&scratch.0, &["gate", "run", "regression"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("$.gates.regression.timeout_seconds"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_missing_manifest_could_not_be_run_and_the_error_names_the_file() {
    let scratch = Scratch::new("no-manifest");

    let output = heph_in(&scratch.0, &["gate", "run", "regression"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains(".athenaeum/manifest.toml"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn the_manifest_can_be_named() {
    let scratch = Scratch::new("named-manifest");
    scratch.write(
        "elsewhere.toml",
        &manifest("", &gate("regression", "[\"true\"]", 30, "")),
    );

    let result = result_of(&heph_in(
        &scratch.0,
        &["gate", "run", "regression", "--manifest", "elsewhere.toml"],
    ));

    assert_eq!(result["outcome"], "passed");
}

#[test]
fn a_command_line_that_cannot_be_understood_is_a_usage_error() {
    let scratch = Scratch::new("usage");

    for args in [
        &["gate"][..],
        &["gate", "run"],
        &["gate", "launch", "regression"],
        &["gate", "run", "one", "two"],
        &["gate", "run", "one", "--changed"],
        &["gate", "run", "one", "--nonsense"],
    ] {
        let output = heph_in(&scratch.0, args);

        assert_eq!(output.status.code(), Some(2), "for {args:?}");
        assert_eq!(stdout(&output), "", "for {args:?}");
        assert!(stderr(&output).contains("Usage: heph "), "for {args:?}");
    }
}
