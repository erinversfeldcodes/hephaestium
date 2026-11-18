//! The command line: `executor run`.

use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "\
Usage: executor run --workload FILE --work-dir DIR --record FILE
                    (--transcript FILE | --agent-command JSON [--agent-env NAME]...)
                    [--source PATH_OR_URL] [--heartbeat-seconds N] [--deliver] [--gh PROGRAM]

Runs one workload. Events are written to standard output, one JSON object to a
line; diagnostics go to standard error. Exits 0 if the workload ran to an
outcome, whatever it was, 1 if it could not be run, and 2 on a usage error.

  --workload FILE     the workload, in the Protobuf JSON mapping
  --work-dir DIR      where the workload's workspace is made, as DIR/<id>
  --record FILE       where the run record is written
  --source SOURCE     where to clone from [default: https://github.com/OWNER/NAME.git]
  --transcript FILE   a recorded transcript to play in place of an agent
                      (development only)
  --agent-command JSON  a command to run as the agent, as a JSON array of
                      strings, whose standard output is events (development
                      only)
  --agent-env NAME    a variable the agent may have, if the executor has it;
                      repeatable. The agent has no others.
  --deliver           when the gates pass, commit the agent's work, push a
                      branch and open the pull request. Needs HEPH_GITHUB_TOKEN,
                      or HEPH_GITHUB_TOKEN_FILE naming a file only you can read;
                      only this step ever holds it.
  --gh PROGRAM        the GitHub command-line tool to use [default: gh]
  --heartbeat-seconds N  how often a heartbeat is written while the agent runs
                      [default: 30]
";

/// What `run` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub workload: PathBuf,
    pub work_dir: PathBuf,
    pub record: PathBuf,
    pub source: Option<String>,
    pub transcript: Option<PathBuf>,
    pub agent_command: Option<Vec<String>>,
    pub agent_env: Vec<String>,
    pub heartbeat: Duration,
    pub deliver: bool,
    pub gh: String,
}

/// How often a heartbeat is written when nothing says otherwise.
pub const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(30);

/// Read the arguments after the program name.
///
/// # Errors
///
/// Returns what is wrong with them, to be printed with the usage.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("run") => {}
        Some(other) => return Err(format!("unknown command: {other}")),
        None => return Err("a command is required".to_owned()),
    }

    let mut workload = None;
    let mut work_dir = None;
    let mut record = None;
    let mut source = None;
    let mut transcript = None;
    let mut agent_command = None;
    let mut agent_env = Vec::new();
    let mut heartbeat = DEFAULT_HEARTBEAT;
    let mut deliver = false;
    let mut gh = "gh".to_owned();
    while let Some(option) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match option.as_str() {
            "--workload" => workload = Some(PathBuf::from(value("--workload")?)),
            "--work-dir" => work_dir = Some(PathBuf::from(value("--work-dir")?)),
            "--record" => record = Some(PathBuf::from(value("--record")?)),
            "--source" => source = Some(value("--source")?),
            "--transcript" => transcript = Some(PathBuf::from(value("--transcript")?)),
            "--agent-command" => {
                let text = value("--agent-command")?;
                let command: Vec<String> = serde_json::from_str(&text).map_err(|error| {
                    format!("--agent-command is not a JSON array of strings: {error}")
                })?;
                if command.is_empty() {
                    return Err("--agent-command is an empty command".to_owned());
                }
                agent_command = Some(command);
            }
            "--deliver" => deliver = true,
            "--gh" => gh = value("--gh")?,
            "--agent-env" => agent_env.push(value("--agent-env")?),
            "--heartbeat-seconds" => {
                let text = value("--heartbeat-seconds")?;
                let seconds: u64 = text
                    .parse()
                    .ok()
                    .filter(|seconds| *seconds > 0)
                    .ok_or("--heartbeat-seconds is a whole number of seconds, at least 1")?;
                heartbeat = Duration::from_secs(seconds);
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }

    if transcript.is_some() && agent_command.is_some() {
        return Err(
            "--transcript and --agent-command are two ways to say the same thing".to_owned(),
        );
    }

    Ok(Options {
        workload: workload.ok_or("--workload is required")?,
        work_dir: work_dir.ok_or("--work-dir is required")?,
        record: record.ok_or("--record is required")?,
        source,
        transcript,
        agent_command,
        agent_env,
        heartbeat,
        deliver,
        gh,
    })
}
