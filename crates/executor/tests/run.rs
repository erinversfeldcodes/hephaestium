//! What `executor run` does with one workload, through the binary, against a
//! local git repository and a recorded transcript in place of an agent.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::{
    SKILL_SHA, Scratch, finished, git, kinds, manifest, run_with, source, transcript, workload,
};
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::{CostUnit, cost};
use serde_json::json;

#[test]
fn a_workload_the_contract_rejects_is_refused_and_nothing_is_created() {
    let scratch = Scratch::new("refused");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let mut bad = workload(&commit);
    bad["repository"]["basebranch"] = json!("main");

    let ran = run_with(scratch, &bad, Some(&transcript()), &[]);

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert!(ran.stderr().contains("basebranch"), "{}", ran.stderr());
    assert_eq!(ran.output.stdout, b"", "events were written");
    assert!(
        !ran.scratch.path("work").exists(),
        "a workspace was created"
    );
    assert!(
        !ran.scratch.path("record.json").exists(),
        "a record was written"
    );
}

#[test]
fn a_transcript_with_a_kind_an_adapter_does_not_produce_is_refused() {
    let scratch = Scratch::new("bad-transcript");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let text = format!(
        "{}{}\n",
        transcript(),
        json!({"finished": {"outcome": "OUTCOME_COMPLETED"}})
    );

    let ran = run_with(scratch, &workload(&commit), Some(&text), &[]);

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert!(ran.stderr().contains("finished"), "{}", ran.stderr());
    assert_eq!(ran.output.stdout, b"");
    assert!(!ran.scratch.path("work").exists());
}

#[test]
fn the_workspace_is_a_clone_at_the_base_commit_and_not_the_tip() {
    let scratch = Scratch::new("clone");
    let (directory, first) = source(&scratch, &manifest(&[]));
    scratch.write("source/later.txt", "added after the base commit\n");
    git(&directory, &["add", "."]);
    git(&directory, &["commit", "-q", "-m", "later"]);

    let ran = run_with(scratch, &workload(&first), Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let Some(Kind::WorkspaceReady(ready)) = events[1].kind.as_ref() else {
        panic!(
            "the second event is not workspace_ready: {:?}",
            kinds(&events)
        );
    };
    assert_eq!(ready.commit, first);
    let workspace = PathBuf::from(&ready.path);
    assert!(
        workspace.starts_with(ran.scratch.path("work")),
        "{workspace:?}"
    );
    assert!(workspace.join("README.md").is_file());
    assert!(
        !workspace.join("later.txt").exists(),
        "the workspace holds a later commit"
    );
    assert_eq!(git(&workspace, &["rev-parse", "HEAD"]), first);
}

#[test]
fn bootstrap_steps_run_in_order_each_from_its_directory() {
    let scratch = Scratch::new("bootstrap-order");
    let steps: [(&[&str], Option<&str>, u64); 3] = [
        (&["sh", "-c", "echo one >> trail"], Some("web"), 30),
        (&["sh", "-c", "echo two >> trail"], Some("web"), 30),
        (&["sh", "-c", "pwd > where"], None, 30),
    ];
    let (_, commit) = source(&scratch, &manifest(&steps));

    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let Some(Kind::WorkspaceReady(ready)) = events[1].kind.as_ref() else {
        panic!("no workspace_ready");
    };
    let workspace = PathBuf::from(&ready.path);
    assert_eq!(
        std::fs::read_to_string(workspace.join("web/trail")).expect("the trail"),
        "one\ntwo\n"
    );
    let where_ = std::fs::read_to_string(workspace.join("where")).expect("where");
    assert_eq!(
        Path::new(where_.trim()).canonicalize().expect("a path"),
        workspace.canonicalize().expect("a path"),
        "a step with no directory runs from the repository root"
    );
}

#[test]
fn a_failing_bootstrap_step_ends_the_workload_as_failed_before_any_agent_event() {
    let scratch = Scratch::new("bootstrap-fails");
    let steps: [(&[&str], Option<&str>, u64); 2] = [
        (&["sh", "-c", "exit 1"], None, 30),
        (&["sh", "-c", "echo never > ran"], None, 30),
    ];
    let (_, commit) = source(&scratch, &manifest(&steps));

    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "a workload that ran to an outcome exits 0");
    let events = ran.events();
    assert_eq!(kinds(&events), ["started", "workspace_ready", "finished"]);
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("exit"), "{reason}");
    assert_eq!(ran.record().outcome, Outcome::Failed as i32);
}

#[test]
fn a_bootstrap_step_past_its_timeout_is_stopped_and_fails_the_workload() {
    let scratch = Scratch::new("bootstrap-hangs");
    let steps: [(&[&str], Option<&str>, u64); 1] = [(&["sleep", "30"], None, 1)];
    let (_, commit) = source(&scratch, &manifest(&steps));

    let started = Instant::now();
    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it was not stopped"
    );
    let events = ran.events();
    assert_eq!(kinds(&events), ["started", "workspace_ready", "finished"]);
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("timed out"), "{reason}");
}

#[test]
fn the_stream_is_gapless_ordered_and_bounded_by_started_and_finished() {
    let scratch = Scratch::new("stream");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let workload = workload(&commit);
    let id = workload["id"].as_str().expect("an id").to_owned();

    let ran = run_with(scratch, &workload, Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    assert_eq!(
        kinds(&events),
        [
            "started",
            "workspace_ready",
            "agent_message",
            "tool_call",
            "tool_result",
            "skill_loaded",
            "cost",
            "cost",
            "cost",
            "agent_message",
            "finished"
        ]
    );
    let mut previous = (0_i64, 0_i32);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, index as u64 + 1, "a gap or a repeat");
        assert_eq!(event.workload_id, id);
        let time = event.time.as_ref().expect("a time");
        assert!(
            (time.seconds, time.nanos) >= previous,
            "time went backwards at {index}"
        );
        previous = (time.seconds, time.nanos);
    }
    assert_eq!(finished(&events).0, Outcome::Completed);
}

#[test]
fn the_record_agrees_with_the_stream() {
    let scratch = Scratch::new("record");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let workload = workload(&commit);

    let ran = run_with(scratch, &workload, Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let record = ran.record();
    assert_eq!(record.outcome, Outcome::Completed as i32);
    assert_eq!(record.event_count, events.len() as u64);
    assert_eq!(record.workload_id, workload["id"].as_str().expect("an id"));
    assert!(record.started_at.is_some() && record.finished_at.is_some());
    assert_ne!(record.executor_version, "");
    // The record says what it ran under: the release it was linked against, as the lock
    // resolved it, and a build named by a commit and not by the workspace version.
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
        .expect("the lock");
    let provenance = record.provenance.as_ref().expect("the run's provenance");
    assert!(provenance.athenaeum_tag.starts_with('v'), "{provenance:?}");
    assert!(
        lock.contains(&format!(
            "athenaeum?tag={}#{}",
            provenance.athenaeum_tag, provenance.athenaeum_commit
        )),
        "the release the record names is the one the lock resolved: {provenance:?}"
    );
    assert_eq!(provenance.executor_commit.len(), 40, "{provenance:?}");
    assert_eq!(
        serde_json::to_value(record.workload.as_ref().expect("the workload")).expect("JSON")["repository"]
            ["baseCommit"],
        json!(commit),
        "the record carries the workload as it ran"
    );

    // Dollars and tokens are totalled apart and never summed across units.
    let mut totals: Vec<(i32, i32, f64)> = record
        .costs
        .iter()
        .map(|c| (c.unit, c.source, c.amount))
        .collect();
    totals.sort_by_key(|t| t.0);
    assert_eq!(
        totals,
        [
            (
                CostUnit::Usd as i32,
                cost::Source::ReportedByRuntime as i32,
                0.75
            ),
            (
                CostUnit::Tokens as i32,
                cost::Source::ReportedByRuntime as i32,
                1200.0
            ),
        ]
    );
    assert_eq!(record.skills.len(), 1);
    assert_eq!(record.skills[0].sha256, SKILL_SHA);
}

#[test]
fn a_value_that_would_be_unsafe_to_use_is_refused_before_anything_is_created() {
    // The commit goes to git, the id names a directory and the manifest path is
    // opened under the workspace, so each must be what it is meant to be.
    for name in ["commit", "id", "manifest"] {
        let scratch = Scratch::new(&format!("unsafe-{name}"));
        let (_, commit) = source(&scratch, &manifest(&[]));
        let mut bad = workload(&commit);
        match name {
            "commit" => bad["repository"]["baseCommit"] = json!("--upload-pack=x"),
            "id" => bad["id"] = json!("../escape"),
            _ => bad["repository"]["manifestPath"] = json!("../../etc/passwd"),
        }

        let ran = run_with(scratch, &bad, Some(&transcript()), &[]);

        assert_eq!(ran.code(), 1, "{name}: {}", ran.stderr());
        assert_eq!(ran.output.stdout, b"", "{name}: events were written");
        assert!(
            !ran.scratch.path("work").exists(),
            "{name}: a workspace was created"
        );
    }
}

#[test]
fn a_workspace_that_already_exists_is_not_reused() {
    let scratch = Scratch::new("exists");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let workload = workload(&commit);
    let id = workload["id"].as_str().expect("an id").to_owned();
    scratch.write(&format!("work/{id}/mine.txt"), "someone else's\n");

    let ran = run_with(scratch, &workload, Some(&transcript()), &[]);

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert_eq!(ran.output.stdout, b"");
    assert!(
        ran.scratch.path(&format!("work/{id}/mine.txt")).is_file(),
        "what was there was touched"
    );
}

#[test]
fn a_source_that_cannot_be_cloned_fails_the_workload() {
    let scratch = Scratch::new("no-source");
    let (_, commit) = source(&scratch, &manifest(&[]));

    let ran = run_with(
        scratch,
        &workload(&commit),
        Some(&transcript()),
        &["--source", "/nowhere/at/all"],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    assert_eq!(kinds(&events), ["started", "finished"]);
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("clone"), "{reason}");
    assert_eq!(ran.record().outcome, Outcome::Failed as i32);
}

#[test]
fn a_manifest_that_is_not_valid_fails_the_workload() {
    let scratch = Scratch::new("bad-manifest");
    let (_, commit) = source(&scratch, "schema_version = 0\nsurprise = true\n");

    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    assert_eq!(kinds(&events), ["started", "workspace_ready", "finished"]);
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("manifest"), "{reason}");
}

#[test]
fn a_relative_work_dir_and_source_mean_relative_to_where_the_executor_was_started() {
    // What the first real run did: it was started from the run's own directory
    // with --work-dir work, and the clone landed in work/work/<id>.
    let scratch = Scratch::new("relative");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let workload = workload(&commit);
    let id = workload["id"].as_str().expect("an id").to_owned();
    scratch.write("workload.json", &workload.to_string());
    scratch.write("transcript.jsonl", &transcript());

    let output = Command::new(env!("CARGO_BIN_EXE_executor"))
        .current_dir(&scratch.0)
        .args([
            "run",
            "--workload",
            "workload.json",
            "--work-dir",
            "work",
            "--record",
            "record.json",
            "--source",
            "source",
            "--transcript",
            "transcript.jsonl",
        ])
        .output()
        .expect("the executor to run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        scratch.path(&format!("work/{id}/README.md")).is_file(),
        "the workspace is not where --work-dir says: {stderr}"
    );
    assert!(
        !scratch.path("work/work").exists(),
        "the clone landed inside the work directory a second time"
    );
    let events = String::from_utf8_lossy(&output.stdout);
    assert!(events.contains("OUTCOME_COMPLETED"), "{events}");
}

#[test]
fn a_bootstrap_step_that_cannot_be_started_says_what_the_system_said() {
    let scratch = Scratch::new("bootstrap-missing");
    let steps: [(&[&str], Option<&str>, u64); 1] = [(&["no-such-program-anywhere"], None, 30)];
    let (_, commit) = source(&scratch, &manifest(&steps));

    let ran = run_with(scratch, &workload(&commit), Some(&transcript()), &[]);

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("could not be started"), "{reason}");
    assert!(reason.contains("No such file or directory"), "{reason}");
}

#[test]
fn a_missing_option_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_executor"))
        .args(["run", "--work-dir", "somewhere"])
        .output()
        .expect("the executor to run");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--workload"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
