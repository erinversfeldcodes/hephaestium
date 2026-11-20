//! A pull request never closes an epic. An epic is a ticket with sub-issues, and what its
//! parts add up to is for a person to judge complete, so a pull request that resolves
//! all the agent was asked to do on it still only refers to it.

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
fn a_pull_request_that_resolves_an_epic_only_refers_to_it() {
    let rig = rig("epic-complete", &manifest(&[]));
    let mut value = workload(&rig.commit);
    value["ticket"]["subIssues"] = json!(3);

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
        body.contains("epic") && body.contains("person"),
        "it says why: {body}"
    );
}

#[test]
fn a_pull_request_that_resolves_a_ticket_with_no_sub_issues_still_closes_it() {
    let rig = rig("epic-none", &manifest(&[]));
    let mut value = workload(&rig.commit);
    value["ticket"]["subIssues"] = json!(0);

    let (_, body) = delivered(rig, &value, "complete\\n");

    assert_eq!(last_line(&body), "Closes #12", "{body}");
    assert!(!body.contains("epic"), "{body}");
}

#[test]
fn a_partial_pull_request_on_an_epic_does_not_add_a_second_reason() {
    let rig = rig("epic-partial", &manifest(&[]));
    let mut value = workload(&rig.commit);
    value["ticket"]["subIssues"] = json!(2);

    let (_, body) = delivered(rig, &value, "partial: the second child remains\\n");

    assert_eq!(last_line(&body), "Refs #12", "{body}");
    assert!(
        body.contains("Not resolved: the second child remains"),
        "{body}"
    );
    assert!(
        !body.contains("is an epic"),
        "the epic note is for a pull request that would otherwise have closed it: {body}"
    );
}
