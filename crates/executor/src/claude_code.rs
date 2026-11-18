//! The Claude Code adapter.
//!
//! Claude Code is started non-interactively with structured output, which is a
//! line of JSON for each thing it does. This adapter says how to start it and
//! turns those lines into events, ignoring every line that is not one it
//! understands, so a kind of line added by a later version cannot fail a run.
//!
//! A run is isolated from the maintainer's own setup as Forge does it: an empty
//! configuration directory of its own, beside the workspace, and settings that
//! turn hooks and attribution off. The run reads the repository's own
//! instructions natively, and authenticates with a token made for it, never the
//! maintainer's login.

use std::path::{Path, PathBuf};

use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::{
    AgentMessage, Cost, CostUnit, LaunchPlan, ToolCall, ToolResult, Workload, cost,
};
use serde_json::{Value, json};

use crate::adapter::Adapter;

/// The tools the agent may use. Anything else is denied, never prompted for, so
/// a run cannot wait on a person.
const ALLOWED_TOOLS: &str = "Read Edit Write Glob Grep Bash";

/// The environment the run may have: where to find programs, and its token.
const ENVIRONMENT: [&str; 2] = ["PATH", "CLAUDE_CODE_OAUTH_TOKEN"];

pub struct ClaudeCode;

/// The run's own configuration directory, beside the workspace so that nothing
/// in it is part of the repository.
fn config_directory(workspace: &Path) -> PathBuf {
    let name = workspace
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    workspace.with_file_name(format!("{name}.claude-config"))
}

/// What the agent is told: the ticket, and that the runtime opens the pull
/// request after its own checks.
fn prompt(workload: &Workload) -> String {
    let ticket = workload.ticket.clone().unwrap_or_default();
    format!(
        "You are working in a clone of the repository, in the current directory, on ticket #{number}: {title}\n\n{body}\n\n\
         Make the change the ticket asks for in this working tree. Do not commit, push or open a pull request: the runtime does that after it has run the repository's checks. When you are done, write a conventional commit message to `.heph-commit-message` in the current directory: a subject line such as `fix: make the reset spec wait for hydration` (a type of feat, fix, docs, chore, ci, test, refactor, perf, build or style, then a colon and a short description), and optionally a blank line and a short body saying what you changed and why. Then say in one short paragraph what you changed.",
        number = ticket.number,
        title = ticket.title,
        body = ticket.body,
    )
}

impl Adapter for ClaudeCode {
    fn prepare(&self, _workload: &Workload, workspace: &Path) -> std::io::Result<()> {
        let directory = config_directory(workspace);
        std::fs::create_dir_all(&directory)?;
        let settings = json!({
            "attribution": {"commit": "", "pr": ""},
            "disableAllHooks": true,
            "apiKeyHelper": "",
        });
        std::fs::write(directory.join("settings.json"), settings.to_string())
    }

    fn plan(&self, workload: &Workload, workspace: &Path) -> LaunchPlan {
        let directory = config_directory(workspace);
        let mut command = vec![
            "env".to_owned(),
            format!("CLAUDE_CONFIG_DIR={}", directory.display()),
            "claude".to_owned(),
            "-p".to_owned(),
            prompt(workload),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
            "--no-session-persistence".to_owned(),
            "--permission-mode".to_owned(),
            "dontAsk".to_owned(),
            "--allowedTools".to_owned(),
            ALLOWED_TOOLS.to_owned(),
            "--settings".to_owned(),
            directory.join("settings.json").display().to_string(),
        ];
        if let Some(runtime) = &workload.runtime
            && !runtime.model.is_empty()
        {
            command.extend(["--model".to_owned(), runtime.model.clone()]);
        }
        if let Some(budget) = workload.limits.as_ref().and_then(|l| l.budget.as_ref())
            && budget.unit == CostUnit::Usd as i32
        {
            command.extend(["--max-budget-usd".to_owned(), budget.amount.to_string()]);
        }
        LaunchPlan {
            command,
            env_names: ENVIRONMENT.iter().map(|n| (*n).to_owned()).collect(),
            working_directory: workspace.display().to_string(),
        }
    }

    fn translate(&self, line: &str) -> Result<Vec<Kind>, String> {
        let value: Value =
            serde_json::from_str(line).map_err(|error| format!("is not JSON: {error}"))?;
        Ok(match value["type"].as_str() {
            Some("assistant") => assistant(&value),
            Some("user") => user(&value),
            Some("result") => result(&value),
            _ => Vec::new(),
        })
    }
}

fn assistant(value: &Value) -> Vec<Kind> {
    let blocks = value["message"]["content"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    blocks
        .iter()
        .filter_map(|block| match block["type"].as_str() {
            Some("text") => {
                let text = block["text"].as_str()?;
                (!text.is_empty()).then(|| {
                    Kind::AgentMessage(AgentMessage {
                        text: text.to_owned(),
                    })
                })
            }
            Some("tool_use") => Some(Kind::ToolCall(ToolCall {
                id: block["id"].as_str().unwrap_or_default().to_owned(),
                name: block["name"].as_str().unwrap_or_default().to_owned(),
                input: serde_json::from_value(block["input"].clone()).ok(),
            })),
            _ => None,
        })
        .collect()
}

fn user(value: &Value) -> Vec<Kind> {
    let blocks = value["message"]["content"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    blocks
        .iter()
        .filter(|block| block["type"].as_str() == Some("tool_result"))
        .map(|block| {
            Kind::ToolResult(ToolResult {
                call_id: block["tool_use_id"].as_str().unwrap_or_default().to_owned(),
                output: text_of(&block["content"]),
                is_error: block["is_error"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// A tool result's text: a string, or the text blocks of a list joined by lines.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The end of a run: what it cost, in dollars and in tokens, each as Claude Code
/// reports it and never converted from the other.
fn result(value: &Value) -> Vec<Kind> {
    let mut costs = Vec::new();
    if let Some(dollars) = value["total_cost_usd"].as_f64() {
        costs.push(cost_of(CostUnit::Usd, dollars));
    }
    let usage = &value["usage"];
    let counts = [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "output_tokens",
    ];
    if counts.iter().any(|name| usage[*name].is_number()) {
        let tokens: f64 = counts
            .iter()
            .map(|name| usage[*name].as_f64().unwrap_or(0.0))
            .sum();
        costs.push(cost_of(CostUnit::Tokens, tokens));
    }
    costs
}

fn cost_of(unit: CostUnit, amount: f64) -> Kind {
    Kind::Cost(Cost {
        unit: unit as i32,
        amount,
        source: cost::Source::ReportedByRuntime as i32,
    })
}
