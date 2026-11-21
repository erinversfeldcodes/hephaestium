//! Where a ticket is judged after its pull request merges, the pull request must not close it.
//! A merge that closes the ticket ends it before the judgement can run, which only judges open
//! tickets, so the claim that the agent resolved the ticket would stand in place of the
//! evidence. The pull request refers to the ticket instead, and says it will be judged.

mod common;

use common::{
    IDENTITY, Rig, agent_script, finished, manifest, rig, run_with_env, stand_in_gh, workload,
};
use contracts::v1alpha1::finished::Outcome;
use serde_json::{Value, json};

fn delivered(rig: Rig, value: &Value, resolution: &str) -> (common::Ran, String) {
    let gh = stand_in_gh(&rig.scratch, 0);
    let script = agent_script(&format!("printf '{resolution}' > .heph-resolution;"));
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let ran = run_with_env(rig.scratch, value, None, &refs, &IDENTITY);
    let body = std::fs::read_to_string(ran.scratch.path("seen/gh-body.txt")).unwrap_or_default();
    (ran, body)
}

fn last_line(body: &str) -> &str {
    body.trim_end().lines().last().unwrap_or_default()
}

#[test]
fn a_pull_request_that_resolves_a_ticket_that_is_judged_afterwards_only_refers_to_it() {
    let rig = rig("judged-complete", &manifest(&[]));
    let mut value = workload(&rig.commit);
    value["judgedAfterMerge"] = json!(true);

    let (ran, body) = delivered(rig, &value, "complete\\n");

    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    assert_eq!(last_line(&body), "Refs #12", "{body}");
    assert!(!body.contains("Closes"), "{body}");
    assert!(
        body.contains("judged") && body.contains("merges"),
        "it says why: {body}"
    );
}

#[test]
fn a_ticket_that_is_not_judged_afterwards_is_still_closed_by_a_pull_request_that_resolves_it() {
    let rig = rig("judged-not", &manifest(&[]));
    let value = workload(&rig.commit);

    let (_, body) = delivered(rig, &value, "complete\\n");

    assert_eq!(last_line(&body), "Closes #12", "{body}");
    assert!(!body.contains("judged"), "{body}");
}

#[test]
fn a_pull_request_that_resolves_part_of_a_ticket_that_is_judged_afterwards_refers_to_it() {
    let rig = rig("judged-partial", &manifest(&[]));
    let mut value = workload(&rig.commit);
    value["judgedAfterMerge"] = json!(true);

    let (_, body) = delivered(rig, &value, "partial: the second half\\n");

    assert_eq!(last_line(&body), "Refs #12", "{body}");
    assert!(body.contains("What remains"), "{body}");
}
