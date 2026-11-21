//! A first run can take minutes, and the base branch can change a file the agent also
//! changed in that time. The agent that made the change is the one that knows what it was
//! for, so it is asked to resolve the difference, in the same run and within the same
//! budget, before the gates judge the result and the pull request is opened. A conflict it
//! cannot resolve fails the run and pushes nothing.

mod common;

use std::path::{Path, PathBuf};

use common::{
    BRANCH, IDENTITY, Rig, agent_script, finished, git, manifest, rig, run_with_env, stand_in_gh,
    workload,
};
use contracts::v1alpha1::Workload;
use contracts::v1alpha1::finished::Outcome;
use executor::adapter::Adapter;
use executor::claude_code::ClaudeCode;
use serde_json::{Value, json};

/// Another change lands on the source's main while the agent works: `text` is written to
/// `path`, replacing it.
fn main_moves(rig: &Rig, path: &str, text: &str) -> String {
    let source = rig.source.display();
    let identity = "-c user.name=other -c user.email=other@example.invalid";
    format!(
        "mkdir -p {source}/web; printf '{text}' > {source}/{path}; \
         git {identity} -C {source} add -A; \
         git {identity} -C {source} commit -q -m 'feat: something another change added';"
    )
}

/// A script for an agent that is run twice: `first` is what the first run does (and it
/// writes the change and the commit message as every first run does), `second` what it does
/// when it is asked again. Each run is recorded in `calls`.
fn script(calls: &Path, first: &str, second: &str) -> String {
    let done = json!({"agentMessage": {"text": "done"}});
    format!(
        "echo x >> {calls}; \
         if [ \"$(wc -l < {calls})\" -ge 2 ]; then {second} echo '{done}'; exit 0; fi; {first}",
        calls = calls.display(),
        first = agent_script(first),
    )
}

fn count(calls: &Path) -> usize {
    std::fs::read_to_string(calls).map_or(0, |text| text.lines().count())
}

struct Outcomes {
    ran: common::Ran,
    source: PathBuf,
    calls: PathBuf,
}

fn run_first(rig: Rig, calls: PathBuf, value: &Value, script: &str) -> Outcomes {
    let gh = stand_in_gh(&rig.scratch, 0);
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let ran = run_with_env(rig.scratch, value, None, &refs, &IDENTITY);
    Outcomes {
        ran,
        source: rig.source,
        calls,
    }
}

const RESOLVE: &str =
    "grep -q '^<<<<<<<' web/changed.txt && printf 'resolved\\n' > web/changed.txt;";

#[test]
fn a_conflict_with_what_the_base_gained_is_resolved_by_the_agent_and_the_run_goes_on() {
    let rig = rig("conflict-resolved", &manifest(&[]));
    let base = rig.commit.clone();
    let calls = rig.scratch.path("agent-calls");
    let moves = main_moves(&rig, "web/changed.txt", "something else\\n");
    let script = script(&calls, &moves, RESOLVE);

    let done = run_first(rig, calls, &workload(&base), &script);

    assert_eq!(done.ran.code(), 0, "{}", done.ran.stderr());
    assert_eq!(finished(&done.ran.events()).0, Outcome::Completed);
    assert_eq!(count(&done.calls), 2, "the agent was asked again, once");
    let main = git(&done.source, &["rev-parse", "main"]);
    assert_eq!(
        git(&done.source, &["rev-parse", &format!("{BRANCH}^")]),
        main,
        "the pull request's one commit is on the base's tip"
    );
    assert_eq!(
        git(
            &done.source,
            &["show", &format!("{BRANCH}:web/changed.txt")]
        ),
        "resolved",
        "what the agent resolved is what was pushed, with no marker left"
    );
}

#[test]
fn the_gates_judge_the_resolved_tree_and_not_the_conflicted_one() {
    // A gate that passes only if the file says what the agent resolved it to.
    let text = manifest(&[])
        + "\n[gates.resolved]\nrun = [\"sh\", \"-c\", \"grep -q resolved web/changed.txt\"]\ntimeout_seconds = 30\nwhen_paths = [\"web/changed.txt\"]\n";
    let rig = rig("conflict-gates", &text);
    let base = rig.commit.clone();
    let calls = rig.scratch.path("agent-calls");
    let moves = main_moves(&rig, "web/changed.txt", "something else\\n");
    let script = script(&calls, &moves, RESOLVE);

    let done = run_first(rig, calls, &workload(&base), &script);

    let (outcome, reason) = finished(&done.ran.events());
    assert_eq!(outcome, Outcome::Completed, "{reason}");
}

#[test]
fn a_change_to_the_same_file_that_merges_cleanly_is_still_put_to_the_agent() {
    // The agent appends to README.md and the base prepends to it: no conflict, but both
    // changed a file, and the agent is to reconfirm its change on what came in.
    let rig = rig("overlap-clean", &manifest(&[]));
    let base = rig.commit.clone();
    let calls = rig.scratch.path("agent-calls");
    let moves = main_moves(&rig, "README.md", "other\\nfirst\\n");
    let first = format!("printf 'agent\\n' >> README.md; {moves}");
    let script = script(&calls, &first, "");

    let done = run_first(rig, calls, &workload(&base), &script);

    assert_eq!(done.ran.code(), 0, "{}", done.ran.stderr());
    assert_eq!(finished(&done.ran.events()).0, Outcome::Completed);
    assert_eq!(count(&done.calls), 2, "the agent was asked to reconfirm");
    assert_eq!(
        git(&done.source, &["show", &format!("{BRANCH}:README.md")]),
        "other\nfirst\nagent",
        "both changes are in what was pushed"
    );
}

#[test]
fn work_that_touches_nothing_the_base_touched_does_not_ask_the_agent_again() {
    let rig = rig("no-overlap-one-call", &manifest(&[]));
    let base = rig.commit.clone();
    let calls = rig.scratch.path("agent-calls");
    let moves = main_moves(&rig, "web/other.txt", "gained\\n");
    let script = script(&calls, &moves, "");

    let done = run_first(rig, calls, &workload(&base), &script);

    assert_eq!(finished(&done.ran.events()).0, Outcome::Completed);
    assert_eq!(count(&done.calls), 1, "the agent was asked once");
}

#[test]
fn a_conflict_the_agent_leaves_in_the_file_fails_the_run_and_pushes_nothing() {
    let rig = rig("conflict-left", &manifest(&[]));
    let base = rig.commit.clone();
    let calls = rig.scratch.path("agent-calls");
    let moves = main_moves(&rig, "web/changed.txt", "something else\\n");
    let script = script(&calls, &moves, "");

    let done = run_first(rig, calls, &workload(&base), &script);

    let (outcome, reason) = finished(&done.ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("web/changed.txt"), "{reason}");
    assert!(reason.contains("conflict"), "{reason}");
    assert_eq!(
        git(&done.source, &["branch", "--list", BRANCH]),
        "",
        "nothing was pushed"
    );
}

#[test]
fn an_agent_that_cannot_be_run_again_fails_the_run_and_pushes_nothing() {
    let rig = rig("conflict-agent-fails", &manifest(&[]));
    let base = rig.commit.clone();
    let calls = rig.scratch.path("agent-calls");
    let moves = main_moves(&rig, "web/changed.txt", "something else\\n");
    let script = script(&calls, &moves, "exit 3;");

    let done = run_first(rig, calls, &workload(&base), &script);

    let (outcome, reason) = finished(&done.ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("exited with code 3"), "{reason}");
    assert_eq!(
        git(&done.source, &["branch", "--list", BRANCH]),
        "",
        "nothing was pushed"
    );
}

#[test]
fn the_second_asking_spends_from_the_budget_the_first_left() {
    // 0.6 and then 0.6 against a budget of 1.0: neither passes it alone.
    let cost = json!({"cost": {"unit": "COST_UNIT_USD", "amount": 0.6, "source": "SOURCE_REPORTED_BY_RUNTIME"}});
    let rig = rig("conflict-budget", &manifest(&[]));
    let base = rig.commit.clone();
    let calls = rig.scratch.path("agent-calls");
    let moves = main_moves(&rig, "web/changed.txt", "something else\\n");
    let first = format!("echo '{cost}'; {moves}");
    let second = format!("echo '{cost}'; sleep 30;");
    let script = script(&calls, &first, &second);
    let mut value = workload(&base);
    value["limits"]["budget"]["amount"] = json!(1.0);

    let done = run_first(rig, calls, &value, &script);

    let (outcome, reason) = finished(&done.ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("budget"), "{reason}");
    assert_eq!(
        git(&done.source, &["branch", "--list", BRANCH]),
        "",
        "nothing was pushed"
    );
}

/// What the agent is told when its first run met a moved base: there is no pull request
/// yet, what came in, which files both changed, and what to do about them.
fn first_run_prompt(conflicts: &[&str]) -> String {
    let mut value = workload("0123456789abcdef0123456789abcdef01234567");
    value["revision"] = json!({
        "pullRequestNumber": "0",
        "pullRequestUrl": "",
        "branch": "",
        "attempt": 1,
        "failures": [],
        "baseMoved": {
            "commits": ["a1b2c3d feat: something another change added"],
            "overlappingFiles": conflicts,
        },
    });
    let workload: Workload = serde_json::from_value(value).expect("a workload");
    let plan = ClaudeCode.plan(&workload, Path::new("/work/w"));
    let at = plan
        .command
        .iter()
        .position(|word| word == "-p")
        .expect("-p");
    plan.command[at + 1].clone()
}

#[test]
fn the_agent_is_told_there_is_no_pull_request_yet_and_what_to_resolve() {
    let prompt = first_run_prompt(&["web/changed.txt"]);

    assert!(!prompt.contains("already open"), "{prompt}");
    assert!(prompt.contains("not been opened"), "{prompt}");
    assert!(prompt.contains("web/changed.txt"), "{prompt}");
    assert!(
        prompt.contains("a1b2c3d feat: something another change added"),
        "{prompt}"
    );
    assert!(
        prompt.contains("<<<<<<<"),
        "it says what a conflict looks like: {prompt}"
    );
    assert!(
        prompt.contains("still exists"),
        "it asks it to reconfirm the problem: {prompt}"
    );
}
