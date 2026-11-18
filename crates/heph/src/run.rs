//! `heph run`: build the workload for a ticket, run the executor on it, show what
//! happens, and say how it ended.
//!
//! A run keeps its own directory, outside the project it was started from, with
//! the workload, the events as they came, the executor's own output, the run
//! record and the workspace. The project is only used to find the repository.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::Value;

use crate::workload::{self, Built};

/// The secrets the executor takes, with the file each is looked for in when it
/// is not given another way, and whether a run cannot go on without it.
const SECRETS: [(&str, &str, bool); 2] = [
    ("HEPH_GITHUB_TOKEN", "github-token", true),
    ("CLAUDE_CODE_OAUTH_TOKEN", "claude-token", false),
];

/// What `run` was asked to do.
pub struct Options {
    pub repository: String,
    pub ticket: i64,
    pub model: String,
    pub timeout_seconds: u64,
    pub budget_usd: f64,
    pub runs_dir: Option<PathBuf>,
    pub executor: Option<PathBuf>,
    /// The commit the project checkout is at, if it is one.
    pub local_head: Option<String>,
}

/// How a run that could be made ended.
pub enum Ended {
    /// The workload completed.
    Completed,
    /// It ended some other way, and the run's summary said how.
    Otherwise,
}

fn environment(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Where `hephaestium`'s own files are kept: `$XDG_CONFIG_HOME` or `~/.config`.
fn config_dir() -> Option<PathBuf> {
    let base = environment("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| environment("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("hephaestium"))
}

/// The runs directory: the option, else `$XDG_STATE_HOME/hephaestium/runs`,
/// else `~/.local/state/hephaestium/runs`.
fn runs_dir(option: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = option.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(path.to_path_buf());
    }
    if let Some(state) = environment("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state).join("hephaestium/runs"));
    }
    environment("HOME")
        .map(|home| PathBuf::from(home).join(".local/state/hephaestium/runs"))
        .ok_or_else(|| "there is no home directory to keep the run in: give --runs-dir".to_owned())
}

/// For each secret not given as a variable or as a `_FILE` variable, the default
/// file offered to the executor through its `_FILE` variable, for that child
/// only. A secret a run cannot go on without, and that is nowhere, is an error
/// that says where to put it.
fn secret_files() -> Result<Vec<(String, PathBuf)>, String> {
    let mut offered = Vec::new();
    for (name, file, required) in SECRETS {
        if environment(name).is_some() || environment(&format!("{name}_FILE")).is_some() {
            continue;
        }
        let default = config_dir().map(|dir| dir.join(file));
        match default.filter(|path| path.is_file()) {
            Some(path) => offered.push((format!("{name}_FILE"), path)),
            None if required => {
                let shown = config_dir().map_or_else(
                    || format!("~/.config/hephaestium/{file}"),
                    |dir| dir.join(file).display().to_string(),
                );
                return Err(format!(
                    "no {name}: set {name}, or {name}_FILE to a file only you can read, or put the token in {shown} (mode 600). For GitHub it is a fine-grained token for this repository with contents and pull-requests write"
                ));
            }
            None => {}
        }
    }
    Ok(offered)
}

/// The executor to run: the option, else the one in `beside` (the directory of
/// this program), else the one on the path.
fn executor_path(option: Option<&Path>, beside: Option<&Path>) -> PathBuf {
    if let Some(path) = option {
        return path.to_path_buf();
    }
    beside
        .map(|dir| dir.join("executor"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("executor"))
}

/// Run a ticket.
///
/// # Errors
///
/// Returns why it could not be run at all: the workload could not be built, a
/// secret is missing, the run's directory could not be made, or the executor
/// could not be started or refused the workload.
pub fn run(options: &Options) -> Result<Ended, String> {
    let Built { workload, warnings } = workload::build(
        &workload::Request {
            repository: &options.repository,
            ticket: options.ticket,
            model: &options.model,
            timeout_seconds: options.timeout_seconds,
            budget_usd: options.budget_usd,
        },
        options.local_head.as_deref(),
    )
    .map_err(|error| error.to_string())?;
    for warning in &warnings {
        eprintln!("heph: warning: {warning}");
    }
    let offered = secret_files()?;

    let directory = runs_dir(options.runs_dir.as_deref())?
        .join(options.repository.replace('/', "-"))
        .join(format!("{}-{}", options.ticket, workload.id));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)
        .map_err(|e| format!("could not make {}: {e}", directory.display()))?;
    let workload_file = directory.join("workload.json");
    let text = serde_json::to_string_pretty(&workload).map_err(|e| e.to_string())?;
    std::fs::write(&workload_file, text + "\n")
        .map_err(|e| format!("could not write {}: {e}", workload_file.display()))?;

    let work = directory.join("work");
    let record = directory.join("record.json");
    let log = directory.join("executor.log");
    let events_file = directory.join("events.jsonl");
    let this_program = std::env::current_exe().ok();
    let executor = executor_path(
        options.executor.as_deref(),
        this_program.as_deref().and_then(Path::parent),
    );

    let log_file = std::fs::File::create(&log)
        .map_err(|e| format!("could not make {}: {e}", log.display()))?;
    let mut command = Command::new(&executor);
    command
        .arg("run")
        .arg("--workload")
        .arg(&workload_file)
        .arg("--work-dir")
        .arg(&work)
        .arg("--record")
        .arg(&record)
        .arg("--deliver")
        .envs(offered)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log_file));
    let mut child = command.spawn().map_err(|error| {
        format!(
            "could not start the executor ({}): {error}",
            executor.display()
        )
    })?;

    let mut events = std::fs::File::create(&events_file)
        .map_err(|e| format!("could not make {}: {e}", events_file.display()))?;
    let started = Instant::now();
    let reader = BufReader::new(child.stdout.take().ok_or("the executor has no output")?);
    for line in reader.lines().map_while(Result::ok) {
        let _ = writeln!(events, "{line}");
        let _ = events.flush();
        if let Some(shown) = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|e| render(&e))
        {
            let seconds = started.elapsed().as_secs();
            println!("[{:02}:{:02}] {shown}", seconds / 60, seconds % 60);
        }
    }
    let status = child
        .wait()
        .map_err(|e| format!("waiting for the executor failed: {e}"))?;
    if status.code() != Some(0) {
        return Err(format!(
            "the executor exited with {}: {}",
            status
                .code()
                .map_or_else(|| "a signal".to_owned(), |c| c.to_string()),
            tail(&log)
        ));
    }

    summarise(&record, &directory)
}

/// The last few lines of a file, for saying what went wrong.
fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(8)..].join("\n")
}

/// Say how the workload ended, from its run record.
fn summarise(record: &Path, directory: &Path) -> Result<Ended, String> {
    let text = std::fs::read_to_string(record).map_err(|e| {
        format!(
            "the executor ended without a run record ({}): {e}",
            record.display()
        )
    })?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("the run record is not JSON: {e}"))?;
    let outcome = value["outcome"]
        .as_str()
        .unwrap_or("OUTCOME_UNSPECIFIED")
        .trim_start_matches("OUTCOME_")
        .to_lowercase();
    println!();
    println!("outcome: {outcome}");
    if let Some(reason) = value["reason"].as_str().filter(|r| !r.is_empty()) {
        println!("reason: {reason}");
    }
    let mut cost = Vec::new();
    for total in value["costs"].as_array().into_iter().flatten() {
        let amount = total["amount"].as_f64().unwrap_or(0.0);
        match total["unit"].as_str() {
            Some("COST_UNIT_USD") => cost.push(format!("${amount:.3}")),
            Some("COST_UNIT_TOKENS") => cost.push(format!("{amount:.0} tokens")),
            Some("COST_UNIT_CREDITS") => cost.push(format!("{amount} credits")),
            _ => {}
        }
    }
    if !cost.is_empty() {
        println!("cost: {}", cost.join(", "));
    }
    if let Some(url) = value["pullRequest"]["url"].as_str() {
        println!("pull request: {url}");
    }
    println!("run: {}", directory.display());
    Ok(if outcome == "completed" {
        Ended::Completed
    } else {
        Ended::Otherwise
    })
}

/// A shortened first line of some text.
fn first_line(text: &str, limit: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() > limit {
        let cut: String = line.chars().take(limit).collect();
        format!("{cut}…")
    } else {
        line.to_owned()
    }
}

/// The line to show for one event, if it is one that matters: where the workspace
/// is, what the agent says and does, each gate and its result, a skill, the pull
/// request. Heartbeats, tool results (unless they are errors), costs and the
/// workload's own start and end are not shown here.
#[must_use]
pub fn render(event: &Value) -> Option<String> {
    if let Some(ready) = event.get("workspaceReady") {
        let commit = ready["commit"].as_str().unwrap_or_default();
        return Some(format!(
            "workspace ready at {}",
            &commit[..commit.len().min(7)]
        ));
    }
    if let Some(message) = event.get("agentMessage") {
        return Some(format!(
            "agent: {}",
            first_line(message["text"].as_str()?, 100)
        ));
    }
    if let Some(call) = event.get("toolCall") {
        let name = call["name"].as_str().unwrap_or("tool");
        let hint = [
            "file_path",
            "command",
            "pattern",
            "path",
            "url",
            "query",
            "description",
        ]
        .iter()
        .find_map(|key| call["input"][*key].as_str())
        .map(|value| first_line(value, 80))
        .unwrap_or_default();
        return Some(format!("tool {name} {hint}").trim_end().to_owned());
    }
    if let Some(result) = event.get("toolResult")
        && result["isError"].as_bool() == Some(true)
    {
        return Some(format!(
            "tool error: {}",
            first_line(result["output"].as_str().unwrap_or_default(), 100)
        ));
    }
    if let Some(finished) = event.get("gateFinished") {
        let name = finished["name"].as_str()?;
        let outcome = finished["result"]["outcome"].as_str().unwrap_or("unknown");
        return Some(format!("gate {name}: {outcome}"));
    }
    if let Some(skill) = event.get("skillLoaded") {
        return Some(format!("skill: {}", skill["name"].as_str()?));
    }
    if let Some(pull_request) = event.get("pullRequestOpened") {
        return Some(format!("pull request: {}", pull_request["url"].as_str()?));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_executor_is_the_option_else_the_one_beside_this_program_else_the_one_on_the_path() {
        let directory =
            std::env::temp_dir().join(format!("heph-run-beside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");

        // Nothing beside it: the one on the path.
        assert_eq!(
            executor_path(None, Some(&directory)),
            PathBuf::from("executor")
        );
        assert_eq!(executor_path(None, None), PathBuf::from("executor"));

        // One beside it is preferred to the path.
        std::fs::write(directory.join("executor"), "").expect("a file");
        assert_eq!(
            executor_path(None, Some(&directory)),
            directory.join("executor")
        );

        // The option beats both.
        let named = Path::new("/somewhere/else/executor");
        assert_eq!(executor_path(Some(named), Some(&directory)), named);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_line_is_cut_at_its_first_newline_and_at_a_limit() {
        assert_eq!(first_line("one\ntwo", 100), "one");
        assert_eq!(first_line("abcdef", 3), "abc…");
        assert_eq!(first_line("  padded  ", 100), "padded");
    }
}
