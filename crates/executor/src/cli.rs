//! The command line: `executor run`.

use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: executor run --workload FILE --work-dir DIR --record FILE
                    [--source PATH_OR_URL] [--transcript FILE]

Runs one workload. Events are written to standard output, one JSON object to a
line; diagnostics go to standard error. Exits 0 if the workload ran to an
outcome, whatever it was, 1 if it could not be run, and 2 on a usage error.

  --workload FILE     the workload, in the Protobuf JSON mapping
  --work-dir DIR      where the workload's workspace is made, as DIR/<id>
  --record FILE       where the run record is written
  --source SOURCE     where to clone from [default: https://github.com/OWNER/NAME.git]
  --transcript FILE   a recorded transcript to play in place of an agent
                      (development only)
";

/// What `run` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub workload: PathBuf,
    pub work_dir: PathBuf,
    pub record: PathBuf,
    pub source: Option<String>,
    pub transcript: Option<PathBuf>,
}

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
    while let Some(option) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match option.as_str() {
            "--workload" => workload = Some(PathBuf::from(value("--workload")?)),
            "--work-dir" => work_dir = Some(PathBuf::from(value("--work-dir")?)),
            "--record" => record = Some(PathBuf::from(value("--record")?)),
            "--source" => source = Some(value("--source")?),
            "--transcript" => transcript = Some(PathBuf::from(value("--transcript")?)),
            other => return Err(format!("unknown option: {other}")),
        }
    }

    Ok(Options {
        workload: workload.ok_or("--workload is required")?,
        work_dir: work_dir.ok_or("--work-dir is required")?,
        record: record.ok_or("--record is required")?,
        source,
        transcript,
    })
}
