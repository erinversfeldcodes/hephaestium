//! The Claude Code adapter: what it asks Claude Code to do, and what it makes of
//! what Claude Code prints. The recorded run in `fixtures/` is a real one,
//! sanitised of paths, hook output and identifiers.

mod common;

use std::path::Path;

use common::workload;
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::{CostUnit, Workload, cost};
use executor::adapter::Adapter;
use executor::claude_code::ClaudeCode;
use serde_json::{Value, json};

const RECORDED: &str = include_str!("fixtures/claude-code/read-file.jsonl");

fn a_workload(edit: impl FnOnce(&mut Value)) -> Workload {
    let mut value = workload("0123456789abcdef0123456789abcdef01234567");
    edit(&mut value);
    serde_json::from_value(value).expect("a workload")
}

fn translate_all(text: &str) -> Vec<Kind> {
    text.lines()
        .flat_map(|line| ClaudeCode.translate(line).expect("a line to translate"))
        .collect()
}

#[test]
fn the_recorded_run_becomes_the_right_events_in_order() {
    let events = translate_all(RECORDED);

    let names: Vec<&str> = events
        .iter()
        .map(|kind| match kind {
            Kind::ToolCall(_) => "tool_call",
            Kind::ToolResult(_) => "tool_result",
            Kind::AgentMessage(_) => "agent_message",
            Kind::Cost(_) => "cost",
            other => panic!("an event the run should not make: {other:?}"),
        })
        .collect();
    assert_eq!(
        names,
        ["tool_call", "tool_result", "agent_message", "cost", "cost"]
    );

    let Kind::ToolCall(call) = &events[0] else {
        unreachable!()
    };
    assert_eq!(call.name, "Read");
    assert_eq!(call.id, "toolu_01PUoniezyME8EMEd6CfcC8b");
    assert_eq!(
        serde_json::to_value(call.input.as_ref().expect("an input")).expect("JSON")["file_path"],
        json!("/work/hello.txt")
    );

    let Kind::ToolResult(result) = &events[1] else {
        unreachable!()
    };
    assert_eq!(result.call_id, call.id);
    assert!(
        result.output.contains("The magic word is pelican."),
        "{}",
        result.output
    );
    assert!(!result.is_error);

    let Kind::AgentMessage(message) = &events[2] else {
        unreachable!()
    };
    assert_eq!(message.text, "The magic word is **pelican**.");
}

#[test]
fn the_result_line_becomes_a_dollar_cost_and_a_token_cost_each_as_reported() {
    let events = translate_all(RECORDED);

    let costs: Vec<_> = events
        .iter()
        .filter_map(|kind| match kind {
            Kind::Cost(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(costs.len(), 2);
    assert_eq!(costs[0].unit, CostUnit::Usd as i32);
    assert!(
        (costs[0].amount - 0.004_811_18).abs() < 1e-12,
        "{}",
        costs[0].amount
    );
    assert_eq!(costs[1].unit, CostUnit::Tokens as i32);
    assert!(
        (costs[1].amount - 62_494.0).abs() < f64::EPSILON,
        "{}",
        costs[1].amount
    );
    for c in costs {
        assert_eq!(c.source, cost::Source::ReportedByRuntime as i32);
    }
}

#[test]
fn thinking_system_and_unknown_lines_produce_nothing_and_a_line_that_is_not_json_fails() {
    for line in [
        r#"{"type":"system","subtype":"hook_started","hook_name":"x"}"#,
        r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":3}"#,
        r#"{"type":"rate_limit_event","rate_limit_info":{}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hm","signature":""}]}}"#,
        r#"{"type":"a_kind_of_line_added_in_a_later_version","anything":[1,2,3]}"#,
        r#"{"no_type_at_all":true}"#,
    ] {
        assert_eq!(
            ClaudeCode.translate(line).expect("to translate"),
            vec![],
            "{line}"
        );
    }

    let error = ClaudeCode
        .translate("this is not json")
        .expect_err("not JSON");
    assert!(error.contains("JSON"), "{error}");
}

#[test]
fn a_tool_result_that_is_an_error_or_holds_a_list_of_text_blocks_is_read_as_such() {
    let line = json!({"type": "user", "message": {"role": "user", "content": [{
        "type": "tool_result", "tool_use_id": "t9", "is_error": true,
        "content": [{"type": "text", "text": "first"}, {"type": "text", "text": "second"}]
    }]}})
    .to_string();

    let events = ClaudeCode.translate(&line).expect("to translate");

    let [Kind::ToolResult(result)] = events.as_slice() else {
        panic!("{events:?}")
    };
    assert_eq!(result.call_id, "t9");
    assert!(result.is_error);
    assert_eq!(result.output, "first\nsecond");
}

#[test]
fn a_message_with_several_blocks_makes_an_event_for_each_in_order() {
    let line = json!({"type": "assistant", "message": {"content": [
        {"type": "text", "text": "I will read it."},
        {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "a"}},
        {"type": "text", "text": "Then edit it."}
    ]}})
    .to_string();

    let events = ClaudeCode.translate(&line).expect("to translate");

    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], Kind::AgentMessage(m) if m.text == "I will read it."));
    assert!(matches!(&events[1], Kind::ToolCall(c) if c.name == "Read"));
    assert!(matches!(&events[2], Kind::AgentMessage(m) if m.text == "Then edit it."));
}

#[test]
fn the_launch_carries_the_prompt_the_model_the_permissions_and_the_tools() {
    let plan = ClaudeCode.plan(&a_workload(|_| {}), Path::new("/work/run-1"));

    let command = &plan.command;
    assert_eq!(command[0], "env");
    assert_eq!(
        command[1], "CLAUDE_CONFIG_DIR=/work/run-1.claude-config",
        "the run's own empty configuration directory, beside the workspace"
    );
    assert_eq!(command[2], "claude");
    assert_eq!(command[3], "-p");
    let prompt = &command[4];
    assert!(
        prompt.contains("Make the reset spec wait for hydration"),
        "{prompt}"
    );
    assert!(
        prompt.contains("The spec clicks Sign in before the page is interactive."),
        "{prompt}"
    );
    assert!(prompt.contains("#12"), "{prompt}");
    assert!(
        prompt.to_lowercase().contains("do not commit"),
        "the agent must be told the runtime opens the pull request: {prompt}"
    );

    let flag = |name: &str| -> &str {
        let at = command
            .iter()
            .position(|a| a == name)
            .unwrap_or_else(|| panic!("no {name}: {command:?}"));
        &command[at + 1]
    };
    assert_eq!(flag("--output-format"), "stream-json");
    assert!(command.iter().any(|a| a == "--verbose"));
    assert!(command.iter().any(|a| a == "--no-session-persistence"));
    assert_eq!(flag("--model"), "claude-opus-4");
    assert_eq!(flag("--permission-mode"), "dontAsk");
    assert_eq!(flag("--allowedTools"), "Read Edit Write Glob Grep Bash");
    assert_eq!(
        flag("--settings"),
        "/work/run-1.claude-config/settings.json"
    );
    assert_eq!(plan.working_directory, "/work/run-1");
}

#[test]
fn a_dollar_budget_is_also_given_to_claude_code_and_another_unit_is_not() {
    let dollars = ClaudeCode.plan(&a_workload(|_| {}), Path::new("/w"));
    let at = dollars
        .command
        .iter()
        .position(|a| a == "--max-budget-usd")
        .expect("a dollar budget");
    assert_eq!(dollars.command[at + 1], "5");

    let tokens = ClaudeCode.plan(
        &a_workload(|w| w["limits"]["budget"]["unit"] = json!("COST_UNIT_TOKENS")),
        Path::new("/w"),
    );
    assert!(
        !tokens.command.iter().any(|a| a == "--max-budget-usd"),
        "{:?}",
        tokens.command
    );
}

#[test]
fn the_plan_names_the_token_and_the_path_and_nothing_else() {
    let plan = ClaudeCode.plan(&a_workload(|_| {}), Path::new("/w"));

    assert_eq!(plan.env_names, ["PATH", "CLAUDE_CODE_OAUTH_TOKEN"]);
    for forbidden in [
        "HOME",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        assert!(
            !plan.env_names.iter().any(|n| n == forbidden),
            "{forbidden}"
        );
    }
}

#[test]
fn preparing_makes_the_empty_configuration_directory_and_settings_that_turn_hooks_and_attribution_off()
 {
    let directory =
        std::env::temp_dir().join(format!("executor-cc-prepare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a directory");
    let workspace = directory.join("run-1");

    ClaudeCode
        .prepare(&a_workload(|_| {}), &workspace)
        .expect("to prepare");

    let config = directory.join("run-1.claude-config");
    assert!(config.is_dir());
    let entries: Vec<_> = std::fs::read_dir(&config)
        .expect("a directory")
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        entries,
        ["settings.json"],
        "the configuration directory holds only the settings"
    );
    let settings: Value = serde_json::from_str(
        &std::fs::read_to_string(config.join("settings.json")).expect("settings"),
    )
    .expect("JSON");
    assert_eq!(settings["disableAllHooks"], json!(true));
    assert_eq!(settings["attribution"], json!({"commit": "", "pr": ""}));
    let _ = std::fs::remove_dir_all(&directory);
}
