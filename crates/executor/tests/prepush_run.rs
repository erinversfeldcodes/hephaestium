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
            "\n[gates.marker]\nrun = [\"sh\", \"-c\", \"echo ran >> {log}; ! grep -q broken web/changed.txt\"]\ntimeout_seconds = 30\n",
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
    let ran = run_with_env(rig.scratch, &value, None, &refs, &IDENTITY);
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
        &[implements("v1"), reads("rust", "approved", None)],
        |_| {},
        true,
    );

    assert_eq!(went.ran.code(), 0, "{}", went.ran.stderr());
    assert_eq!(finished(&went.ran.events()).0, Outcome::Completed);
    assert_eq!(went.starts, 2, "the implementing agent and one reader");
    assert_eq!(pushed(&went), [BRANCH]);
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
}

#[test]
fn a_reader_that_leaves_no_verdict_is_no_approval() {
    let went = go(
        setup("prepush-silent"),
        &[implements("v1"), say(), say()],
        |_| {},
        true,
    );

    let (outcome, reason) = finished(&went.ran.events());
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(reason.contains("no verdict"), "{reason}");
    assert_eq!(
        went.starts, 3,
        "read again in the next round, with nothing to revise by"
    );
    assert_eq!(pushed(&went).len(), 0);
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
