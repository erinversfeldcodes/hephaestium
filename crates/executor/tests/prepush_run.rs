//! A run that reads its own change under the lenses it needs before it pushes anything.
//!
//! The agent here is one shell script that behaves differently each time it is started, by
//! counting its starts: the first is the implementing agent, then a reader for each lens,
//! then the implementing agent again if something blocked, and so on. So each test says,
//! start by start, what the agents did, and checks what the executor made of it.

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use common::{
    BRANCH, IDENTITY, MESSAGE, Ran, Rig, finished, git, kinds, manifest, rig, run_with_env,
    stand_in_gh, workload,
};
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::step::Kind as StepKind;
use serde_json::{Value, json};

const RUST: &str = "---\nname: rust\n---\nLook at ownership.";
const STYLE: &str = "---\nname: style\napplies: always\nblocking: false\n---\nLook at style.";

/// The manifest: one component that names the `rust` pack, and a gate that notes each time it
/// runs and fails if the file holds `broken`.
fn manifest_for(scratch: &Path) -> String {
    manifest(&[]).replace("root = \".\"\n", "root = \".\"\npack = \"rust\"\n")
        + &format!(
            "\n[gates.marker]\nrun = [\"sh\", \"-c\", \"echo ran >> {log}; if grep -q broken web/changed.txt; then echo 'the marker gate found the word broken' >&2; exit 1; fi\"]\ntimeout_seconds = 30\n",
            log = scratch.join("gates.log").display()
        )
}

fn say() -> String {
    format!("echo '{}'", json!({"agentMessage": {"text": "done"}}))
}

fn cost(amount: f64) -> String {
    format!(
        "echo '{}'",
        json!({"cost": {"unit": "COST_UNIT_USD", "amount": amount, "source": "SOURCE_REPORTED_BY_RUNTIME"}})
    )
}

/// The implementing agent's first start: it makes the change and writes its message.
fn implements(content: &str) -> String {
    format!(
        "mkdir -p web; echo {content} > web/changed.txt; printf '{}' > .heph-commit-message; {}",
        MESSAGE.replace('\n', "\\n"),
        say()
    )
}

/// The implementing agent started again to fix what blocked.
fn fixes(content: &str) -> String {
    format!("echo {content} > web/changed.txt; {}", say())
}

/// A reader's verdict under `lens`, for whatever commit the workspace is at.
fn reads(lens: &str, kind: &str, blocking: Option<&str>) -> String {
    let findings = blocking.map_or_else(
        || json!([]),
        |title| {
            json!([{
                "severity": "P1",
                "blocking": true,
                "title": title,
                "evidence": "web/changed.txt:1 shows it.",
                "revision": "Change it."
            }])
        },
    );
    let verdict = json!({
        "schema_version": 0,
        "lens": lens,
        "commit": "COMMIT",
        "verdict": kind,
        "findings": findings,
    });
    format!(
        "c=$(git rev-parse HEAD); printf '%s' '{verdict}' | sed \"s/COMMIT/$c/\" > .heph-verdict.json; {}",
        say()
    )
}

/// An agent that counts its starts and does the `n`th of these on the `n`th.
fn agent(root: &Path, starts: &[String]) -> String {
    let mut text = format!(
        "mkdir -p {root}; n=$(cat {root}/n 2>/dev/null || echo 0); n=$((n+1)); echo $n > {root}/n\ncase $n in\n",
        root = root.display()
    );
    for (at, start) in starts.iter().enumerate() {
        let _ = writeln!(text, "{}) {start} ;;", at + 1);
    }
    // A start nobody expected fails loudly.
    text.push_str("*) echo unexpected start >&2; exit 9 ;;\nesac\n");
    text
}

struct Setup {
    rig: Rig,
    /// Where the agent counts its starts.
    root: PathBuf,
    /// Where the gate notes each run.
    log: PathBuf,
}

fn setup(name: &str) -> Setup {
    // The manifest names a file in the scratch directory, so its path is needed before the
    // directory is made: it is the one `Scratch::new` will make.
    let scratch_path = std::env::temp_dir().join(format!("executor-{name}-{}", std::process::id()));
    let rig = rig(name, &manifest_for(&scratch_path));
    Setup {
        root: rig.scratch.path("agent"),
        log: rig.scratch.path("gates.log"),
        rig,
    }
}

fn lens(pack: &str, document: &str) -> Value {
    json!({"pack": pack, "document": document})
}

struct Went {
    ran: Ran,
    source: PathBuf,
    /// How many times the agent was started.
    starts: u32,
    /// How many times the gate ran.
    gate_runs: usize,
}

/// Run the workload with the agent's starts, delivering unless `deliver` is false.
fn go(setup: Setup, starts: &[String], edit: impl FnOnce(&mut Value), deliver: bool) -> Went {
    let Setup { rig, root, log } = setup;
    let mut value = workload(&rig.commit);
    value["lenses"] = json!([lens("rust", RUST)]);
    value["prepushRounds"] = json!(2);
    edit(&mut value);
    let script = agent(&root, starts);
    let gh = stand_in_gh(&rig.scratch, 0);
    let mut arguments = vec![
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
    ];
    if deliver {
        arguments.push("--deliver".to_owned());
    }
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let source = rig.source.clone();
    // The wait between a reader's tries is for an outage; the tests do not wait.
    let mut environment: Vec<(&str, &str)> = IDENTITY.to_vec();
    environment.push(("HEPH_READER_RETRY_WAIT_MS", "0"));
    let ran = run_with_env(rig.scratch, &value, None, &refs, &environment);
    let starts = std::fs::read_to_string(root.join("n"))
        .map_or(0, |text| text.trim().parse().expect("a count"));
    let gate_runs = std::fs::read_to_string(log).map_or(0, |text| text.lines().count());
    Went {
        ran,
        source,
        starts,
        gate_runs,
    }
}

fn pushed(went: &Went) -> Vec<String> {
    git(&went.source, &["branch", "--list", "heph/*"])
        .lines()
        .map(|line| line.trim_start_matches(['*', ' ']).to_owned())
        .collect()
}

#[test]
fn a_change_every_lens_approves_is_pushed_once_with_a_step_for_each_agent() {
    let went = go(
        setup("prepush-approved"),
        &[
            implements("v1"),
            format!("sleep 1; {}", reads("rust", "approved", None)),
        ],
        |_| {},
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(finished(&went.ran.events()).0, Outcome::Completed);
    let record = went.ran.record();
    assert!(
        (1_000..30_000).contains(&record.review_ms),
        "the reader slept a second, and that time is review's and not the agent's: {}",
        record.review_ms
    );
    assert_eq!(went.starts, 2, "the implementing agent and one reader");
    assert_eq!(pushed(&went), [BRANCH]);
    let names = kinds(&went.ran.events());
    assert_eq!(
        &names[names.len() - 3..],
        ["delivery_began", "pull_request_opened", "finished"],
        "said once, after the reading and before the push: {names:?}"
    );
    assert_eq!(
        names
            .iter()
            .filter(|name| **name == "delivery_began")
            .count(),
        1,
        "{names:?}"
    );
    assert_eq!(
        git(
            &went.source,
            &["rev-list", "--count", &format!("main..{BRANCH}")]
        ),
        "1"
    );

    let steps = went.ran.record().steps;
    let shape: Vec<(i32, &str, i32, &str)> = steps
        .iter()
        .map(|s| (s.kind, s.lens.as_str(), s.round, s.verdict.as_str()))
        .collect();
    assert_eq!(
        shape,
        [
            (StepKind::Implementation as i32, "", 0, ""),
            (StepKind::Review as i32, "rust", 1, "approved"),
        ]
    );
}

#[test]
fn what_blocks_is_fixed_and_read_again_and_what_is_pushed_is_the_fixed_commit_alone() {
    let went = go(
        setup("prepush-revised"),
        &[
            implements("v1"),
            reads("rust", "needs_revision", Some("an unchecked unwrap")),
            fixes("v2"),
            reads("rust", "approved", None),
        ],
        |_| {},
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(went.starts, 4);
    assert_eq!(pushed(&went), [BRANCH]);
    assert_eq!(
        git(
            &went.source,
            &["rev-list", "--count", &format!("main..{BRANCH}")]
        ),
        "1",
        "the fix is part of the one commit, not a second"
    );
    assert_eq!(
        git(
            &went.source,
            &["show", &format!("{BRANCH}:web/changed.txt")]
        ),
        "v2"
    );
    assert_eq!(
        went.gate_runs, 2,
        "the gates ran again on what the agent changed"
    );

    let kinds_seen: Vec<(i32, i32)> = went
        .ran
        .record()
        .steps
        .iter()
        .map(|s| (s.kind, s.round))
        .collect();
    assert_eq!(
        kinds_seen,
        [
            (StepKind::Implementation as i32, 0),
            (StepKind::Review as i32, 1),
            (StepKind::Revision as i32, 1),
            (StepKind::Review as i32, 2),
        ]
    );
}

#[test]
fn rounds_used_up_with_a_lens_still_blocking_pushes_nothing_and_says_what_was_found() {
    let went = go(
        setup("prepush-exhausted"),
        &[
            implements("v1"),
            reads("rust", "needs_revision", Some("an unchecked unwrap")),
            fixes("v2"),
            reads("rust", "needs_revision", Some("a second unwrap")),
        ],
        |_| {},
        true,
    );

    let (outcome, reason) = finished(&went.ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(reason.contains("a second unwrap"), "{reason}");
    assert!(reason.contains("rust"), "{reason}");
    assert!(reason.contains("Nothing was pushed"), "{reason}");
    assert_eq!(went.starts, 4);
    assert_eq!(pushed(&went).len(), 0, "{:?}", pushed(&went));
    assert!(
        !went.ran.scratch.path("seen/gh-args.txt").exists(),
        "no pull request was opened"
    );
    assert!(
        went.ran.scratch.path("work").exists(),
        "the workspace is kept to be looked at"
    );
    assert!(!kinds(&went.ran.events()).contains(&"pull_request_opened"));
    assert!(
        !kinds(&went.ran.events()).contains(&"delivery_began"),
        "a run that pushes nothing never says delivery began"
    );
}

#[test]
fn a_reader_that_leaves_no_verdict_is_tried_three_times_and_is_then_no_approval() {
    let went = go(
        setup("prepush-silent"),
        &[implements("v1"), say(), say(), say()],
        |value| {
            value["prepushRounds"] = json!(1);
        },
        true,
    );

    let (outcome, reason) = finished(&went.ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(reason.contains("no verdict"), "{reason}");
    assert_eq!(
        went.starts, 4,
        "the implementing agent, and the reader three times"
    );
    assert_eq!(pushed(&went).len(), 0);
    let tries: Vec<(i32, i32)> = went
        .ran
        .record()
        .steps
        .iter()
        .filter(|s| s.kind == StepKind::Review as i32)
        .map(|s| (s.round, i32::from(s.verdict.is_empty())))
        .collect();
    assert_eq!(
        tries,
        [(1, 1), (1, 1), (1, 1)],
        "each try is a step of round 1"
    );
}

#[test]
fn a_reader_that_could_not_run_is_tried_again_in_the_same_round_and_no_round_is_used() {
    let went = go(
        setup("prepush-recovers"),
        &[
            implements("v1"),
            "echo 'API Error: Can not reach the API server' >&2; exit 1".to_owned(),
            reads("rust", "approved", None),
        ],
        |value| {
            value["prepushRounds"] = json!(1);
        },
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(
        went.starts, 3,
        "the implementing agent, the reader that failed, the reader again"
    );
    assert_eq!(pushed(&went), [BRANCH]);
    let reviews: Vec<(i32, String)> = went
        .ran
        .record()
        .steps
        .iter()
        .filter(|s| s.kind == StepKind::Review as i32)
        .map(|s| (s.round, s.verdict.clone()))
        .collect();
    assert_eq!(
        reviews,
        [(1, String::new()), (1, "approved".to_owned())],
        "both tries are round 1, and only one round was allowed"
    );
}

#[test]
fn a_reader_that_recovers_still_leaves_the_rounds_to_fix_what_it_finds() {
    let went = go(
        setup("prepush-recovers-then-blocks"),
        &[
            implements("v1"),
            "exit 1".to_owned(),
            reads("rust", "needs_revision", Some("an unchecked unwrap")),
            fixes("v2"),
            reads("rust", "approved", None),
        ],
        |_| {},
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(went.starts, 5);
    assert_eq!(
        git(
            &went.source,
            &["show", &format!("{BRANCH}:web/changed.txt")]
        ),
        "v2"
    );
}

#[test]
fn a_verdict_that_was_given_is_never_tried_again() {
    let went = go(
        setup("prepush-not-retried"),
        &[
            implements("v1"),
            reads("rust", "needs_revision", Some("an unchecked unwrap")),
        ],
        |value| {
            value["prepushRounds"] = json!(1);
        },
        true,
    );

    assert_eq!(finished(&went.ran.events()).0, Outcome::Failed);
    assert_eq!(
        went.starts, 2,
        "a reader's judgement is not a failure to retry"
    );
}

#[test]
fn a_reader_that_changed_a_file_is_not_tried_again() {
    let went = go(
        setup("prepush-editor"),
        &[
            implements("v1"),
            format!("echo edited >> web/changed.txt; {}", say()),
        ],
        |value| {
            value["prepushRounds"] = json!(1);
        },
        true,
    );

    let (outcome, reason) = finished(&went.ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(reason.contains("changed the workspace"), "{reason}");
    assert_eq!(went.starts, 2, "a reader that edits is not a flake");
}

#[test]
fn a_lens_that_does_not_block_is_read_and_never_stops_the_push() {
    let went = go(
        setup("prepush-advisory"),
        &[
            implements("v1"),
            reads("style", "needs_revision", Some("a long line")),
            reads("rust", "approved", None),
        ],
        |value| {
            value["lenses"] = json!([lens("style", STYLE), lens("rust", RUST)]);
        },
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(
        went.starts, 3,
        "both were read, and nothing was revised for the advisory one"
    );
    assert_eq!(pushed(&went), [BRANCH]);
}

#[test]
fn a_reader_that_spends_the_budget_ends_the_run_without_a_push() {
    let went = go(
        setup("prepush-budget"),
        &[
            format!("{}; {}", cost(0.6), implements("v1")),
            format!("{}; {}", cost(0.6), reads("rust", "approved", None)),
        ],
        |value| {
            value["limits"]["budget"]["amount"] = json!(1.0);
        },
        true,
    );

    let (outcome, reason) = finished(&went.ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(reason.contains("budget"), "{reason}");
    assert_eq!(pushed(&went).len(), 0);
}

#[test]
fn what_each_step_spent_is_told_apart_and_adds_up_to_what_the_run_spent() {
    let went = go(
        setup("prepush-costs"),
        &[
            format!("{}; {}", cost(0.25), implements("v1")),
            format!(
                "{}; {}",
                cost(0.125),
                reads("rust", "needs_revision", Some("an unchecked unwrap"))
            ),
            format!("{}; {}", cost(0.5), fixes("v2")),
            format!("{}; {}", cost(0.0625), reads("rust", "approved", None)),
        ],
        |value| {
            value["limits"]["budget"]["amount"] = json!(10.0);
        },
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    let record = went.ran.record();
    let spent: Vec<f64> = record
        .steps
        .iter()
        .map(|step| step.costs.iter().map(|c| c.amount).sum())
        .collect();
    assert_eq!(spent, [0.25, 0.125, 0.5, 0.0625]);
    let total: f64 = record.costs.iter().map(|c| c.amount).sum();
    assert!(
        (total - spent.iter().sum::<f64>()).abs() < 1e-9,
        "{total} against {spent:?}"
    );
}

#[test]
fn a_fix_that_breaks_a_gate_ends_the_run_without_a_push() {
    let went = go(
        setup("prepush-gate"),
        &[
            implements("v1"),
            reads("rust", "needs_revision", Some("an unchecked unwrap")),
            fixes("broken"),
        ],
        |_| {},
        true,
    );

    let (outcome, reason) = finished(&went.ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(reason.contains("gate marker"), "{reason}");
    assert_eq!(went.starts, 3, "nothing read the broken change");
    assert_eq!(pushed(&went).len(), 0);
}

#[test]
fn one_round_reads_once_and_never_revises() {
    let went = go(
        setup("prepush-one-round"),
        &[
            implements("v1"),
            reads("rust", "needs_revision", Some("an unchecked unwrap")),
        ],
        |value| {
            value["prepushRounds"] = json!(1);
        },
        true,
    );

    let (outcome, reason) = finished(&went.ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert_eq!(went.starts, 2);
    assert_eq!(pushed(&went).len(), 0);
}

#[test]
fn a_change_no_lens_applies_to_is_pushed_without_being_read() {
    let went = go(
        setup("prepush-no-lens"),
        &[implements("v1")],
        |value| {
            // The library has lenses, but nothing the change touches names one of them.
            value["lenses"] = json!([lens("css", "---\nname: css\n---\nLook at css.")]);
        },
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(went.starts, 1);
    assert_eq!(pushed(&went), [BRANCH]);
}

#[test]
fn a_run_that_delivers_nothing_reads_nothing() {
    let went = go(
        setup("prepush-no-deliver"),
        &[implements("v1")],
        |_| {},
        false,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(went.starts, 1, "there is no push to read for");
    assert_eq!(went.ran.record().steps.len(), 0);
}

#[test]
fn a_gate_that_fails_is_fixed_by_the_agent_and_the_fixed_change_is_read_and_pushed() {
    let went = go(
        setup("gate-fixed"),
        &[
            implements("broken"),
            format!("{}; {}", cost(0.125), fixes("v2")),
            reads("rust", "approved", None),
        ],
        |value| {
            value["limits"]["budget"]["amount"] = json!(10.0);
        },
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(finished(&went.ran.events()).0, Outcome::Completed);
    assert_eq!(
        went.starts, 3,
        "the implementing agent, again for the gate, and a reader"
    );
    assert_eq!(
        went.gate_runs, 2,
        "the gates ran again on what the agent changed"
    );
    assert_eq!(pushed(&went), [BRANCH]);
    assert_eq!(
        git(
            &went.source,
            &["show", &format!("{BRANCH}:web/changed.txt")]
        ),
        "v2"
    );
    assert_eq!(
        git(
            &went.source,
            &["rev-list", "--count", &format!("main..{BRANCH}")]
        ),
        "1"
    );

    let steps = went.ran.record().steps;
    let shape: Vec<(i32, i32)> = steps.iter().map(|s| (s.kind, s.round)).collect();
    assert_eq!(
        shape,
        [
            (StepKind::Implementation as i32, 0),
            (StepKind::Revision as i32, 1),
            (StepKind::Review as i32, 1),
        ]
    );
    let fixing: f64 = steps[1].costs.iter().map(|c| c.amount).sum();
    assert!(
        (fixing - 0.125).abs() < 1e-9,
        "the fix turn's own cost: {fixing}"
    );
}

#[test]
fn a_gate_that_still_fails_after_the_rounds_ends_the_run_with_what_it_printed() {
    let went = go(
        setup("gate-never-fixed"),
        &[implements("broken"), fixes("broken"), fixes("broken")],
        |_| {},
        true,
    );

    let (outcome, reason) = finished(&went.ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(reason.contains("gate marker"), "{reason}");
    assert!(
        reason.contains("the marker gate found the word broken"),
        "what the gate printed is in the reason: {reason}"
    );
    assert_eq!(
        went.starts, 3,
        "two fixes, as many as there are rounds, and no reader"
    );
    assert_eq!(pushed(&went).len(), 0);
    assert!(
        went.ran
            .record()
            .steps
            .iter()
            .all(|s| s.kind != StepKind::Review as i32),
        "nothing was read, because the gates never passed"
    );
}

#[test]
fn a_gate_that_fails_is_not_given_to_the_agent_when_the_run_reads_nothing_before_the_push() {
    let went = go(
        setup("gate-no-reading"),
        &[implements("broken"), fixes("v2")],
        |value| {
            value["lenses"] = json!([]);
            value["prepushRounds"] = json!(0);
        },
        true,
    );

    let (outcome, reason) = finished(&went.ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert_eq!(went.starts, 1, "the run ended, as it did before");
}

fn token_cost(amount: f64, parts: Option<[f64; 4]>) -> String {
    let mut cost = json!({"unit": "COST_UNIT_TOKENS", "amount": amount, "source": "SOURCE_REPORTED_BY_RUNTIME"});
    if let Some([input, output, creation, read]) = parts {
        cost["breakdown"] =
            json!({"input": input, "output": output, "cacheCreation": creation, "cacheRead": read});
    }
    format!("echo '{}'", json!({ "cost": cost }))
}

#[test]
fn the_parts_of_a_token_cost_are_summed_in_the_record_and_each_step_carries_only_its_own() {
    let went = go(
        setup("parts-summed"),
        &[
            format!(
                "{}; {}; {}",
                token_cost(100.0, Some([10.0, 20.0, 30.0, 40.0])),
                token_cost(50.0, Some([5.0, 5.0, 10.0, 30.0])),
                implements("v1")
            ),
            format!(
                "{}; {}",
                token_cost(200.0, Some([20.0, 40.0, 60.0, 80.0])),
                reads("rust", "approved", None)
            ),
        ],
        |value| {
            // Tokens are the unit of this run's budget, so nothing here is dollars.
            value["limits"]["budget"]["unit"] = json!("COST_UNIT_TOKENS");
            value["limits"]["budget"]["amount"] = json!(1_000_000.0);
        },
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    let record = went.ran.record();

    let total = record
        .costs
        .iter()
        .find(|c| c.unit == contracts::v1alpha1::CostUnit::Tokens as i32)
        .expect("a token total");
    assert!((total.amount - 350.0).abs() < 1e-9, "{}", total.amount);
    let parts = total.breakdown.as_ref().expect("the parts, summed");
    assert!((parts.input - 35.0).abs() < 1e-9, "{parts:?}");
    assert!((parts.output - 65.0).abs() < 1e-9, "{parts:?}");
    assert!((parts.cache_creation - 100.0).abs() < 1e-9, "{parts:?}");
    assert!((parts.cache_read - 150.0).abs() < 1e-9, "{parts:?}");
    let sum = parts.input + parts.output + parts.cache_creation + parts.cache_read;
    assert!(
        (sum - total.amount).abs() < 1e-9,
        "the parts add up to the total"
    );

    // The implementation's step holds its two events' parts; the reader's holds its own.
    let implementation = record.steps[0].costs[0]
        .breakdown
        .as_ref()
        .expect("its parts");
    assert!(
        (implementation.cache_read - 70.0).abs() < 1e-9,
        "{implementation:?}"
    );
    let reading = record.steps[1].costs[0]
        .breakdown
        .as_ref()
        .expect("its parts");
    assert!((reading.cache_read - 80.0).abs() < 1e-9, "{reading:?}");
    assert!((reading.input - 20.0).abs() < 1e-9, "{reading:?}");
}

#[test]
fn a_run_whose_runtime_gave_no_parts_has_none_and_one_that_gave_some_of_them_does_not_claim_all() {
    let went = go(
        setup("parts-missing"),
        &[
            format!("{}; {}", token_cost(100.0, None), implements("v1")),
            format!(
                "{}; {}",
                token_cost(50.0, Some([5.0, 5.0, 10.0, 30.0])),
                reads("rust", "approved", None)
            ),
        ],
        |value| {
            value["limits"]["budget"]["unit"] = json!("COST_UNIT_TOKENS");
            value["limits"]["budget"]["amount"] = json!(1_000_000.0);
        },
        true,
    );

    let record = went.ran.record();
    assert!(
        record.steps[0].costs[0].breakdown.is_none(),
        "a step whose runtime gave no parts has none"
    );
    let total = &record.costs[0];
    assert!((total.amount - 150.0).abs() < 1e-9, "{}", total.amount);
    let parts = total.breakdown.as_ref().expect("the parts there were");
    let sum = parts.input + parts.output + parts.cache_creation + parts.cache_read;
    assert!(
        (sum - 50.0).abs() < 1e-9,
        "only what was given, and no more: {sum}"
    );
}

/// The step events of a run, in order, as (what, kind, round, lens, verdict). What each is: "start"
/// or "end".
fn step_events(ran: &Ran) -> Vec<(&'static str, i32, i32, String, String)> {
    ran.events()
        .iter()
        .filter_map(|event| match event.kind.as_ref()? {
            contracts::v1alpha1::event::Kind::StepStarted(step) => Some((
                "start",
                step.kind,
                step.round,
                step.lens.clone(),
                String::new(),
            )),
            contracts::v1alpha1::event::Kind::StepFinished(step) => Some((
                "end",
                step.kind,
                step.round,
                step.lens.clone(),
                step.verdict.clone(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn the_events_say_which_agent_is_running_around_each_agents_own_events() {
    use contracts::v1alpha1::step_started::Kind as Step;
    let went = go(
        setup("events-order"),
        &[implements("v1"), reads("rust", "approved", None)],
        |_| {},
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(
        step_events(&went.ran),
        [
            (
                "start",
                Step::Implementation as i32,
                0,
                String::new(),
                String::new()
            ),
            (
                "end",
                Step::Implementation as i32,
                0,
                String::new(),
                String::new()
            ),
            (
                "start",
                Step::Review as i32,
                1,
                "rust".to_owned(),
                String::new()
            ),
            (
                "end",
                Step::Review as i32,
                1,
                "rust".to_owned(),
                "approved".to_owned()
            ),
        ]
    );

    // The reader's own events fall between its start and its end.
    let events = went.ran.events();
    let at = |wanted: &str, nth: usize| {
        kinds(&events)
            .iter()
            .enumerate()
            .filter(|(_, name)| **name == wanted)
            .nth(nth)
            .map(|(index, _)| index)
            .expect("an event of that kind")
    };
    let (started, finished) = (at("step_started", 1), at("step_finished", 1));
    let spoke = at("agent_message", 1);
    assert!(started < spoke && spoke < finished, "{:?}", kinds(&events));
}

#[test]
fn a_revision_for_what_blocked_and_each_round_are_told() {
    use contracts::v1alpha1::step_started::Kind as Step;
    let went = go(
        setup("events-revision"),
        &[
            implements("v1"),
            reads("rust", "needs_revision", Some("an unchecked unwrap")),
            fixes("v2"),
            reads("rust", "approved", None),
        ],
        |_| {},
        true,
    );

    let starts: Vec<(i32, i32, String)> = step_events(&went.ran)
        .into_iter()
        .filter(|event| event.0 == "start")
        .map(|event| (event.1, event.2, event.3))
        .collect();
    assert_eq!(
        starts,
        [
            (Step::Implementation as i32, 0, String::new()),
            (Step::Review as i32, 1, "rust".to_owned()),
            (Step::Revision as i32, 1, String::new()),
            (Step::Review as i32, 2, "rust".to_owned()),
        ]
    );
    let first_verdict = step_events(&went.ran)
        .into_iter()
        .find(|event| event.0 == "end" && event.1 == Step::Review as i32)
        .expect("a review ended");
    assert_eq!(first_verdict.4, "needs_revision");
}

#[test]
fn each_try_of_a_reader_that_could_not_run_is_a_start_and_an_end_with_no_verdict_for_the_first() {
    use contracts::v1alpha1::step_started::Kind as Step;
    let went = go(
        setup("events-retry"),
        &[
            implements("v1"),
            "exit 1".to_owned(),
            reads("rust", "approved", None),
        ],
        |value| {
            value["prepushRounds"] = json!(1);
        },
        true,
    );

    let reviews: Vec<(&str, String)> = step_events(&went.ran)
        .into_iter()
        .filter(|event| event.1 == Step::Review as i32)
        .map(|event| (event.0, event.4))
        .collect();
    assert_eq!(
        reviews,
        [
            ("start", String::new()),
            ("end", String::new()),
            ("start", String::new()),
            ("end", "approved".to_owned()),
        ]
    );
}

#[test]
fn a_gate_that_is_handed_back_is_told_as_a_revision() {
    use contracts::v1alpha1::step_started::Kind as Step;
    let went = go(
        setup("events-gate"),
        &[
            implements("broken"),
            fixes("v2"),
            reads("rust", "approved", None),
        ],
        |_| {},
        true,
    );

    let kinds_started: Vec<i32> = step_events(&went.ran)
        .into_iter()
        .filter(|event| event.0 == "start")
        .map(|event| event.1)
        .collect();
    assert_eq!(
        kinds_started,
        [
            Step::Implementation as i32,
            Step::Revision as i32,
            Step::Review as i32
        ]
    );
}

#[test]
fn a_run_that_reads_nothing_before_the_push_has_no_step_events() {
    let went = go(
        setup("events-none"),
        &[implements("v1")],
        |value| {
            value["lenses"] = json!([]);
            value["prepushRounds"] = json!(0);
        },
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(step_events(&went.ran).len(), 0);
}

#[test]
fn the_kinds_of_a_step_event_mean_what_they_do_in_the_run_record() {
    use contracts::v1alpha1::{step, step_started};
    assert_eq!(
        step::Kind::Implementation as i32,
        step_started::Kind::Implementation as i32
    );
    assert_eq!(step::Kind::Review as i32, step_started::Kind::Review as i32);
    assert_eq!(
        step::Kind::Revision as i32,
        step_started::Kind::Revision as i32
    );
    assert_eq!(
        step::Kind::Unspecified as i32,
        step_started::Kind::Unspecified as i32
    );
}
