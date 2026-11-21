//! A first run can take minutes, and the base branch can move in that time. Before
//! its pull request is opened the work is carried onto the base branch's current
//! tip, so the pull request is not opened against ground that has changed. What the
//! base gained is not the agent's change. A change that touches what the base also
//! touched is not carried over mechanically: the agent resolves it, as the conflict tests show.

mod common;

use common::{
    BRANCH, IDENTITY, Rig, agent_script, finished, git, manifest, rig, run_with_env, stand_in_gh,
    workload,
};
use contracts::v1alpha1::finished::Outcome;
use serde_json::{Value, json};

/// What the agent runs while it works: another change lands on the source's main.
fn main_moves_during_the_run(rig: &Rig, path: &str, text: &str) -> String {
    let source = rig.source.display();
    let identity = "-c user.name=other -c user.email=other@example.invalid";
    format!(
        "mkdir -p {source}/web; printf '{text}' > {source}/{path}; \
         git {identity} -C {source} add -A; \
         git {identity} -C {source} commit -q -m 'feat: something another change added';"
    )
}

fn run_first(rig: Rig, value: &Value, script: &str) -> (common::Ran, std::path::PathBuf) {
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
    (ran, rig.source)
}

#[test]
fn work_is_carried_onto_what_the_base_gained_while_the_agent_ran() {
    let rig = rig("catch-up-moved", &manifest(&[]));
    let base = rig.commit.clone();
    let script = agent_script(&main_moves_during_the_run(
        &rig,
        "web/other.txt",
        "gained\\n",
    ));

    let (ran, source) = run_first(rig, &workload(&base), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
    let main = git(&source, &["rev-parse", "main"]);
    assert_ne!(main, base, "the base branch did move");
    assert_eq!(
        git(&source, &["rev-parse", &format!("{BRANCH}^")]),
        main,
        "the pull request's one commit is on the base's tip, not on the commit the run began at"
    );
    assert_eq!(
        git(&source, &["show", &format!("{BRANCH}:web/other.txt")]),
        "gained",
        "what the base gained is in the branch"
    );
    assert_eq!(
        git(&source, &["show", &format!("{BRANCH}:web/changed.txt")]),
        "changed",
        "and so is the agent's change"
    );
}

#[test]
fn a_base_that_has_not_moved_leaves_the_work_where_it_began() {
    let rig = rig("catch-up-unmoved", &manifest(&[]));
    let base = rig.commit.clone();

    let (ran, source) = run_first(rig, &workload(&base), &agent_script(""));

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(git(&source, &["rev-parse", &format!("{BRANCH}^")]), base);
}

#[test]
fn what_the_base_gained_is_not_counted_as_the_agents_change() {
    // A gate that fails, and runs only for a change to web/other.txt: the file the
    // base gains and the agent never touches.
    let text = manifest(&[])
        + "\n[gates.other]\nrun = [\"sh\", \"-c\", \"exit 1\"]\ntimeout_seconds = 30\nwhen_paths = [\"web/other.txt\"]\n";
    let rig = rig("catch-up-not-counted", &text);
    let base = rig.commit.clone();
    let script = agent_script(&main_moves_during_the_run(
        &rig,
        "web/other.txt",
        "gained\\n",
    ));

    let (ran, _) = run_first(rig, &workload(&base), &script);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(
        outcome,
        Outcome::Completed,
        "main's file was counted as the agent's change: {reason}"
    );
}
