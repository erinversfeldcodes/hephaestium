//! What the executor does with a repository's gates: runs them after the agent,
//! relays each result, and ends a workload as completed only if none failed.

mod common;

use std::time::{Duration, Instant};

use common::{Scratch, finished, kinds, manifest, run_with, source, transcript, workload};
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::{Event, GateFinished};
use serde_json::{Value, json};

/// A gate in TOML: its name, command, timeout and anything more.
fn gate(name: &str, command: &str, timeout: u64, more: &str) -> String {
    format!(
        "\n[gates.{name}]\nrun = [\"sh\", \"-c\", {command:?}]\ntimeout_seconds = {timeout}\n{more}"
    )
}

fn gate_results(events: &[Event]) -> Vec<(String, Value)> {
    events
        .iter()
        .filter_map(|event| match event.kind.as_ref() {
            Some(Kind::GateFinished(GateFinished { name, result })) => Some((
                name.clone(),
                serde_json::to_value(result.as_ref().expect("a result")).expect("JSON"),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn gates_run_after_the_agent_and_each_is_relayed_with_its_result() {
    let scratch = Scratch::new("gates-pass");
    let text = manifest(&[]) + &gate("lint", "exit 0", 30, "") + &gate("unit", "exit 0", 30, "");
    let (_, commit) = source(&scratch, &text);

    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let kinds = kinds(&events);
    let agent_end = kinds
        .iter()
        .rposition(|k| *k == "agent_message")
        .expect("agent");
    assert_eq!(
        kinds[agent_end + 1..],
        [
            "gate_started",
            "gate_finished",
            "gate_started",
            "gate_finished",
            "finished"
        ],
        "gates run after the agent's last event, one at a time"
    );
    let results = gate_results(&events);
    assert_eq!(
        results.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        ["lint", "unit"]
    );
    for (name, result) in &results {
        assert_eq!(result["outcome"], "passed", "{name}: {result}");
        assert_eq!(result["gate"], json!(name));
    }
    assert_eq!(finished(&events).0, Outcome::Completed);

    let record = ran.record();
    assert_eq!(record.outcome, Outcome::Completed as i32);
    assert_eq!(
        record
            .gates
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>(),
        ["lint", "unit"]
    );
}

#[test]
fn a_failing_gate_fails_the_workload_names_it_and_the_others_still_run() {
    let scratch = Scratch::new("gates-fail");
    let text = manifest(&[])
        + &gate("a-lint", "exit 0", 30, "")
        + &gate("b-unit", "exit 1", 30, "")
        + &gate("c-docs", "exit 0", 30, "");
    let (_, commit) = source(&scratch, &text);

    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "a workload that ran to an outcome exits 0");
    let events = ran.events();
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("b-unit"), "{reason}");
    assert!(
        !reason.contains("a-lint") && !reason.contains("c-docs"),
        "{reason}"
    );
    let results = gate_results(&events);
    assert_eq!(results.len(), 3, "every gate ran, though one failed");
    assert_eq!(results[1].1["outcome"], "failed");
    assert_eq!(results[2].1["outcome"], "passed");
    assert_eq!(ran.record().outcome, Outcome::Failed as i32);
    assert_eq!(ran.record().gates.len(), 3);
}

#[test]
fn a_gate_that_times_out_is_stopped_and_fails_the_workload() {
    let scratch = Scratch::new("gates-timeout");
    let text = manifest(&[]) + &gate("slow", "sleep 30", 1, "");
    let (_, commit) = source(&scratch, &text);

    let started = Instant::now();
    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it was not stopped"
    );
    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Failed);
    assert_eq!(gate_results(&events)[0].1["outcome"], "timed_out");
}

#[test]
fn a_gate_that_cannot_be_run_fails_the_workload_and_names_what_is_missing() {
    let scratch = Scratch::new("gates-missing");
    let text = manifest(&[])
        + &gate(
            "needy",
            "touch ran",
            30,
            "\n[gates.needy.requires]\nHEPH_EXECUTOR_TEST_UNSET = \"secret\"\n",
        );
    let (_, commit) = source(&scratch, &text);

    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("needy"), "{reason}");
    assert!(reason.contains("HEPH_EXECUTOR_TEST_UNSET"), "{reason}");
    assert!(
        gate_results(&events).is_empty(),
        "a gate that was not run has no result"
    );
}

#[test]
fn a_gate_applies_to_what_changed_in_the_workspace() {
    let scratch = Scratch::new("gates-applies");
    // A bootstrap step adds a file under docs/, which is a change to the
    // workspace; nothing is added under src/.
    let steps: [(&[&str], Option<&str>, u64); 1] = [(
        &["sh", "-c", "mkdir -p docs && echo new > docs/new.md"],
        None,
        30,
    )];
    let text = manifest(&steps)
        + &gate(
            "docs-check",
            "touch docs-ran",
            30,
            "when_paths = [\"docs/**\"]\n",
        )
        + &gate(
            "src-check",
            "touch src-ran",
            30,
            "when_paths = [\"src/**\"]\n",
        );
    let (_, commit) = source(&scratch, &text);

    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let results = gate_results(&events);
    assert_eq!(results[0].0, "docs-check");
    assert_eq!(results[0].1["outcome"], "passed");
    assert_eq!(results[1].0, "src-check");
    assert_eq!(results[1].1["outcome"], "skipped");
    assert_eq!(
        finished(&events).0,
        Outcome::Completed,
        "a skipped gate is not a failure"
    );
}

#[test]
fn gates_and_bootstrap_steps_run_in_the_workspace_through_the_toolchain_wrapper() {
    let scratch = Scratch::new("gates-wrapper");
    let steps: [(&[&str], Option<&str>, u64); 1] =
        [(&["sh", "-c", "echo $WRAPPED > bootstrap-wrapped"], None, 30)];
    let text = manifest(&steps)
        + "\n[toolchain]\nwrapper = [\"env\", \"WRAPPED=yes\"]\n"
        + &gate(
            "where",
            "echo $WRAPPED > gate-wrapped; pwd > gate-where",
            30,
            "",
        );
    let (_, commit) = source(&scratch, &text);

    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    assert_eq!(finished(&events).0, Outcome::Completed, "{events:?}");
    let Some(Kind::WorkspaceReady(ready)) = events[1].kind.as_ref() else {
        panic!("no workspace_ready");
    };
    let workspace = std::path::PathBuf::from(&ready.path);
    for file in ["bootstrap-wrapped", "gate-wrapped"] {
        assert_eq!(
            std::fs::read_to_string(workspace.join(file))
                .expect(file)
                .trim(),
            "yes",
            "{file}: the command did not run through the wrapper"
        );
    }
    let where_ = std::fs::read_to_string(workspace.join("gate-where")).expect("where");
    assert_eq!(
        std::path::Path::new(where_.trim())
            .canonicalize()
            .expect("a path"),
        workspace.canonicalize().expect("a path"),
        "a gate runs from the workspace root"
    );
}
