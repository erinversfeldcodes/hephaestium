//! The value of a secret the executor holds never appears in what it emits: not in
//! an event, and not in the run record. Whatever the agent or a gate prints, the
//! value is replaced by a marker that names the secret.

mod common;

use common::{IDENTITY, Rig, Scratch, agent_script, finished, run_with_env, source, stand_in_gh};
use contracts::v1alpha1::finished::Outcome;
use serde_json::{Value, json};

const CLAUDE: &str = "sk-ant-oat01-the-claude-token";
const GATE: &str = "a-pixabay-key-value";
const MARKER: &str = "[redacted: CLAUDE_CODE_OAUTH_TOKEN]";

/// A shell command that prints this JSON line, with each `@TOKEN@` in it the Claude token the
/// agent was given.
fn prints(line: &Value) -> String {
    let text = line
        .to_string()
        .replace("@TOKEN@", "'\"$CLAUDE_CODE_OAUTH_TOKEN\"'");
    format!("printf '%s\\n' '{text}';")
}

/// An agent that prints the Claude token it was given in a message, in a tool call's input and in
/// a tool result, as one of its own lines each.
fn printing_the_token() -> String {
    [
        json!({"agentMessage": {"text": "my token is @TOKEN@"}}),
        json!({"toolCall": {"id": "t1", "name": "Bash", "input": {"command": "curl -H @TOKEN@ x"}}}),
        json!({"toolResult": {"callId": "t1", "output": "CLAUDE_CODE_OAUTH_TOKEN=@TOKEN@"}}),
    ]
    .iter()
    .map(prints)
    .collect::<Vec<_>>()
    .join(" ")
}

/// A repository with a gate that gives the secret it declares in the report it prints.
fn rig_with_gate(name: &str) -> Rig {
    let scratch = Scratch::new(name);
    let text = r#"schema_version = 0

[components.app]
root = "."
paths = ["**"]

[gates.noisy]
run = ["sh", "-c", "printf '{\"key\":\"%s\"}' \"$PIXABAY_API_KEY\""]
result = "json"
timeout_seconds = 30

[gates.noisy.requires]
PIXABAY_API_KEY = "secret"
"#;
    let (source_dir, commit) = source(&scratch, text);
    Rig {
        scratch,
        commit,
        source: source_dir,
    }
}

fn run(rig: Rig, script: &str, environment: &[(&str, &str)]) -> common::Ran {
    let gh = stand_in_gh(&rig.scratch, 0);
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--agent-env".to_owned(),
        "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        "--gh".to_owned(),
        gh.display().to_string(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let mut all: Vec<(&str, &str)> = IDENTITY
        .iter()
        .copied()
        .filter(|(name, _)| *name != "HEPH_GITHUB_TOKEN")
        .collect();
    all.extend_from_slice(environment);
    run_with_env(
        rig.scratch,
        &common::workload(&rig.commit),
        None,
        &refs,
        &all,
    )
}

/// What the executor wrote to its output, as it was written.
fn stdout(ran: &common::Ran) -> String {
    String::from_utf8(ran.output.stdout.clone()).expect("UTF-8")
}

/// The record, as it was written.
fn record_text(ran: &common::Ran) -> String {
    std::fs::read_to_string(ran.scratch.path("record.json")).expect("a record")
}

/// The one event of this kind, as JSON.
fn event_of(ran: &common::Ran, kind: &str) -> Value {
    let all: Vec<Value> = stdout(ran)
        .lines()
        .map(|line| serde_json::from_str(line).expect("an event"))
        .collect();
    all.into_iter()
        .find(|event| event.get(kind).is_some())
        .unwrap_or_else(|| panic!("no {kind} event"))
}

#[test]
fn a_claude_token_the_agent_prints_is_a_marker_in_a_message_a_tool_call_and_a_tool_result() {
    let rig = common::rig("redact-claude", &common::manifest(&[]));
    let script = agent_script(&printing_the_token());

    let ran = run(rig, &script, &[("CLAUDE_CODE_OAUTH_TOKEN", CLAUDE)]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = stdout(&ran);
    assert!(
        !events.contains(CLAUDE),
        "an event holds the token:\n{events}"
    );
    let message = event_of(&ran, "agentMessage");
    assert_eq!(
        message["agentMessage"]["text"],
        format!("my token is {MARKER}")
    );
    let call = event_of(&ran, "toolCall");
    assert_eq!(
        call["toolCall"]["input"]["command"],
        format!("curl -H {MARKER} x")
    );
    let result = event_of(&ran, "toolResult");
    assert_eq!(
        result["toolResult"]["output"],
        format!("CLAUDE_CODE_OAUTH_TOKEN={MARKER}")
    );
    assert!(!record_text(&ran).contains(CLAUDE));
}

#[test]
fn a_gate_secret_a_gate_prints_is_a_marker_in_the_events_and_the_record() {
    let rig = rig_with_gate("redact-gate");
    let script = agent_script("");

    let ran = run(rig, &script, &[("HEPH_GATE_SECRET_PIXABAY_API_KEY", GATE)]);

    let events = stdout(&ran);
    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    let done = event_of(&ran, "gateFinished");
    assert_eq!(
        done["gateFinished"]["result"]["report"]["key"], "[redacted: PIXABAY_API_KEY]",
        "{events}"
    );
    assert!(
        !events.contains(GATE),
        "an event holds the secret:\n{events}"
    );
    assert!(!record_text(&ran).contains(GATE));
}

#[test]
fn a_token_in_the_reason_the_run_ends_on_is_a_marker_in_the_event_and_the_record() {
    // A line that is not an event, with the token where the executor's complaint about it quotes
    // it, is what the run ends on.
    let rig = common::rig("redact-reason", &common::manifest(&[]));
    let script = prints(&json!({"@TOKEN@": 1}));

    let ran = run(rig, &script, &[("CLAUDE_CODE_OAUTH_TOKEN", CLAUDE)]);

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed, "{}", ran.stderr());
    assert!(
        reason.contains(MARKER),
        "the token was not in the reason, so this proves nothing: {reason}"
    );
    assert_eq!(ran.record().reason, reason);
    assert!(!record_text(&ran).contains(CLAUDE), "{}", record_text(&ran));
    assert!(!stdout(&ran).contains(CLAUDE));
}

#[test]
fn a_secret_in_the_question_the_agent_asks_is_a_marker_in_the_event_and_the_record() {
    let rig = common::rig("redact-question", &common::manifest(&[]));
    let script = "printf 'Should I use %s for it?' \"$CLAUDE_CODE_OAUTH_TOKEN\" > .heph-question;";

    let ran = run(rig, script, &[("CLAUDE_CODE_OAUTH_TOKEN", CLAUDE)]);

    let expected = format!("Should I use {MARKER} for it?");
    let events = ran.events();
    let Some(contracts::v1alpha1::event::Kind::Finished(done)) =
        events.last().and_then(|e| e.kind.as_ref())
    else {
        panic!("no finished event: {}", ran.stderr());
    };
    assert_eq!(done.outcome, Outcome::Blocked as i32, "{}", ran.stderr());
    assert_eq!(done.question, expected, "the event");
    assert_eq!(ran.record().question, expected, "the record");
    assert!(!record_text(&ran).contains(CLAUDE), "{}", record_text(&ran));
    assert!(!stdout(&ran).contains(CLAUDE));
}

#[test]
fn text_that_holds_only_part_of_a_secret_is_left_as_it_was() {
    let rig = common::rig("redact-part", &common::manifest(&[]));
    let (half, longer) = (&CLAUDE[..12], format!("{CLAUDE}-and-more"));
    let script = [
        json!({"agentMessage": {"text": format!("only {half} of it")}}),
        json!({"agentMessage": {"text": format!("or {longer}")}}),
    ]
    .iter()
    .map(prints)
    .collect::<Vec<_>>()
    .join(" ");

    let ran = run(
        rig,
        &agent_script(&script),
        &[("CLAUDE_CODE_OAUTH_TOKEN", CLAUDE)],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let events = stdout(&ran);
    assert!(events.contains(&format!("only {half} of it")), "{events}");
    // A longer text that holds the whole value is a text that holds the value.
    assert!(
        events.contains(&format!("or {MARKER}-and-more")),
        "{events}"
    );
}

#[test]
fn a_value_shorter_than_eight_characters_is_not_redacted() {
    let rig = common::rig("redact-short", &common::manifest(&[]));
    let script = prints(&json!({"agentMessage": {"text": "the value is @TOKEN@"}}));

    let ran = run(
        rig,
        &agent_script(&script),
        &[("CLAUDE_CODE_OAUTH_TOKEN", "short-1")],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let message = event_of(&ran, "agentMessage");
    // Seven characters.
    assert_eq!(message["agentMessage"]["text"], "the value is short-1");
}

#[test]
fn a_token_in_what_the_executor_says_about_itself_is_a_marker_in_its_log() {
    // The agent's program cannot be started, and the executor says why in its own log, naming the
    // program, which here holds the token.
    let rig = common::rig("redact-log", &common::manifest(&[]));
    let program = format!("/nonexistent/{CLAUDE}");

    let gh = stand_in_gh(&rig.scratch, 0);
    let arguments = [
        "--agent-command".to_owned(),
        json!([program]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let ran = run_with_env(
        rig.scratch,
        &common::workload(&rig.commit),
        None,
        &refs,
        &[("CLAUDE_CODE_OAUTH_TOKEN", CLAUDE)],
    );

    let log = ran.stderr();
    assert!(
        log.contains(&format!("/nonexistent/{MARKER}")),
        "the log did not name the program, so this proves nothing:\n{log}"
    );
    assert!(!log.contains(CLAUDE), "the log holds the token:\n{log}");
    assert!(!stdout(&ran).contains(CLAUDE));
    assert!(!record_text(&ran).contains(CLAUDE));
}
