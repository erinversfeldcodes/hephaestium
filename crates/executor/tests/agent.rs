//! What the executor does with an agent that runs as a process: the environment
//! it holds, the limits it runs under, and what becomes of the workload when it
//! goes wrong.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Ran, Scratch, finished, kinds, manifest, run_with, run_with_env, source, workload};
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use serde_json::{Value, json};

/// The arguments that run `script` as the stub agent.
fn agent(script: &str) -> Vec<String> {
    vec![
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
    ]
}

fn as_args(arguments: &[String]) -> Vec<&str> {
    arguments.iter().map(String::as_str).collect()
}

/// A line of shell that prints one agent event.
fn say(text: &str) -> String {
    format!("echo '{}'", json!({"agentMessage": {"text": text}}))
}

fn cost(unit: &str, amount: f64) -> String {
    format!(
        "echo '{}'",
        json!({"cost": {"unit": unit, "amount": amount, "source": "SOURCE_REPORTED_BY_RUNTIME"}})
    )
}

/// A manifest with one gate that leaves a file if it runs.
fn with_a_gate() -> String {
    manifest(&[])
        + "\n[gates.marker]\nrun = [\"sh\", \"-c\", \"touch gate-ran\"]\ntimeout_seconds = 30\n"
}

fn ran(name: &str, edit: impl FnOnce(&mut Value), script: &str, extra: &[&str]) -> Ran {
    let scratch = Scratch::new(name);
    let (_, commit) = source(&scratch, &with_a_gate());
    let mut value = workload(&commit);
    edit(&mut value);
    let mut arguments = agent(script);
    arguments.extend(extra.iter().map(|a| (*a).to_owned()));
    run_with(scratch, &value, None, &as_args(&arguments))
}

fn workspace_of(ran: &Ran) -> PathBuf {
    let events = ran.events();
    let Some(Kind::WorkspaceReady(ready)) = events[1].kind.as_ref() else {
        panic!("no workspace_ready: {:?}", kinds(&events));
    };
    PathBuf::from(&ready.path)
}

fn no_gate_ran(ran: &Ran) {
    assert!(
        !workspace_of(ran).join("gate-ran").exists(),
        "a gate ran, though the agent did not finish its work"
    );
    assert!(
        !kinds(&ran.events()).contains(&"gate_started"),
        "gates were announced"
    );
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("kill")
        .success()
}

#[test]
fn the_agent_holds_only_the_variables_its_adapter_names() {
    let scratch = Scratch::new("agent-env");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let script = r#"printf '{"agentMessage":{"text":"%s|%s"}}\n' "${HEPH_TEST_NAMED:-none}" "${HEPH_TEST_UNNAMED:-none}""#;
    let mut arguments = agent(script);
    arguments.extend(["--agent-env".to_owned(), "HEPH_TEST_NAMED".to_owned()]);

    let ran = run_with_env(
        scratch,
        &workload(&commit),
        None,
        &as_args(&arguments),
        &[("HEPH_TEST_NAMED", "yes"), ("HEPH_TEST_UNNAMED", "secret")],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let Some(Kind::AgentMessage(message)) = events[2].kind.as_ref() else {
        panic!("{:?}", kinds(&events));
    };
    assert_eq!(message.text, "yes|none");
}

#[test]
fn the_agent_runs_in_the_workspace() {
    let ran = ran(
        "agent-cwd",
        |_| {},
        r#"printf '{"agentMessage":{"text":"%s"}}\n' "$(pwd)""#,
        &[],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let Some(Kind::AgentMessage(message)) = events[2].kind.as_ref() else {
        panic!("{:?}", kinds(&events));
    };
    assert_eq!(
        PathBuf::from(&message.text).canonicalize().expect("a path"),
        workspace_of(&ran).canonicalize().expect("a path")
    );
}

#[test]
fn heartbeats_are_written_while_the_agent_is_quiet_and_the_sequence_stays_gapless() {
    let script = format!("{}; sleep 3; {}", say("one"), say("two"));
    let ran = ran(
        "agent-heartbeat",
        |_| {},
        &script,
        &["--heartbeat-seconds", "1"],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = ran.events();
    let kinds = kinds(&events);
    let first = kinds
        .iter()
        .position(|k| *k == "agent_message")
        .expect("one");
    let second = kinds
        .iter()
        .rposition(|k| *k == "agent_message")
        .expect("two");
    let beats = kinds[first..second]
        .iter()
        .filter(|k| **k == "heartbeat")
        .count();
    assert!(beats >= 2, "heartbeats between the two messages: {beats}");
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, index as u64 + 1);
    }
}

#[test]
fn a_timeout_stops_the_agent_and_what_it_started_ends_timed_out_and_runs_no_gates() {
    let started = Instant::now();
    let ran = ran(
        "agent-timeout",
        |w| w["limits"]["timeout"] = json!("1s"),
        &format!(
            "{}; sleep 60 >/dev/null 2>&1 & echo $! > child.pid; wait",
            say("working")
        ),
        &[],
    );

    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it was not stopped"
    );
    let events = ran.events();
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::TimedOut);
    assert!(reason.contains("timeout"), "{reason}");
    assert_eq!(ran.record().outcome, Outcome::TimedOut as i32);
    no_gate_ran(&ran);
    let pid = std::fs::read_to_string(workspace_of(&ran).join("child.pid"))
        .expect("a pid")
        .trim()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(&pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(&pid), "the agent's child {pid} outlived it");
}

#[test]
fn a_spent_budget_stops_the_agent_and_ends_failed_naming_the_budget() {
    let started = Instant::now();
    let script = format!(
        "{}; {}; sleep 60",
        cost("COST_UNIT_USD", 0.6),
        cost("COST_UNIT_USD", 0.6)
    );
    let ran = ran(
        "agent-budget",
        |w| w["limits"]["budget"]["amount"] = json!(1.0),
        &script,
        &[],
    );

    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it was not stopped"
    );
    let events = ran.events();
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("budget"), "{reason}");
    assert_eq!(
        kinds(&events).iter().filter(|k| **k == "cost").count(),
        2,
        "the cost that passed the budget is recorded"
    );
    no_gate_ran(&ran);
}

#[test]
fn a_cost_in_another_unit_does_not_count_against_the_budget() {
    let script = format!("{}; {}", cost("COST_UNIT_TOKENS", 1.0e9), say("done"));
    let ran = ran(
        "agent-units",
        |w| w["limits"]["budget"]["amount"] = json!(1.0),
        &script,
        &[],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(finished(&ran.events()).0, Outcome::Completed);
}

#[test]
fn an_agent_that_exits_non_zero_or_cannot_start_fails_the_workload_and_runs_no_gates() {
    let ran = ran(
        "agent-exit",
        |_| {},
        &format!("{}; exit 3", say("oops")),
        &[],
    );
    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("code 3"), "{reason}");
    no_gate_ran(&ran);

    let scratch = Scratch::new("agent-missing");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let missing = run_with(
        scratch,
        &workload(&commit),
        None,
        &[
            "--agent-command",
            &json!(["no-such-program-anywhere"]).to_string(),
        ],
    );
    let (outcome, reason) = finished(&missing.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("started"), "{reason}");
}

#[test]
fn a_line_that_is_not_an_event_or_is_the_executors_own_kind_fails_the_workload() {
    let ran_a = ran("agent-notjson", |_| {}, "echo 'not json'", &[]);
    let (outcome, reason) = finished(&ran_a.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("line 1"), "{reason}");
    no_gate_ran(&ran_a);

    let own = format!(
        "echo '{}'",
        json!({"finished": {"outcome": "OUTCOME_COMPLETED"}})
    );
    let ran_b = ran("agent-own-kind", |_| {}, &own, &[]);
    let (outcome, reason) = finished(&ran_b.events());
    assert_eq!(outcome, Outcome::Failed);
    assert!(reason.contains("finished"), "{reason}");
    assert_eq!(
        kinds(&ran_b.events())
            .iter()
            .filter(|k| **k == "finished")
            .count(),
        1,
        "the agent wrote a finished event into the stream"
    );
}

#[test]
fn a_workload_with_no_end_is_refused_and_nothing_is_created() {
    for (name, edit) in [
        (
            "no-timeout",
            (|w: &mut Value| {
                w["limits"]
                    .as_object_mut()
                    .expect("limits")
                    .remove("timeout");
            }) as fn(&mut Value),
        ),
        ("no-limits", |w| {
            w.as_object_mut().expect("a workload").remove("limits");
        }),
        ("zero-timeout", |w| w["limits"]["timeout"] = json!("0s")),
        ("no-budget", |w| {
            w["limits"]
                .as_object_mut()
                .expect("limits")
                .remove("budget");
        }),
        ("zero-budget", |w| {
            w["limits"]["budget"]["amount"] = json!(0);
        }),
        ("no-budget-unit", |w| {
            w["limits"]["budget"]
                .as_object_mut()
                .expect("budget")
                .remove("unit");
        }),
    ] {
        let ran = ran(&format!("agent-end-{name}"), edit, &say("x"), &[]);
        assert_eq!(ran.code(), 1, "{name}: {}", ran.stderr());
        assert_eq!(ran.output.stdout, b"", "{name}: events were written");
        assert!(
            !ran.scratch.path("work").exists(),
            "{name}: a workspace was created"
        );
    }
}

#[test]
fn running_with_no_agent_selected_is_refused() {
    let scratch = Scratch::new("agent-none");
    let (_, commit) = source(&scratch, &manifest(&[]));

    let ran = run_with(scratch, &workload(&commit), None, &[]);

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert!(ran.stderr().contains("agent"), "{}", ran.stderr());
    assert_eq!(ran.output.stdout, b"");
}
