//! Running a command for at most a given time, and stopping everything it
//! started.
//!
//! A command runs in a process group of its own. When its deadline passes, or
//! waiting for it fails, the whole group is killed, so a command cannot leave a
//! process behind that outlives it.

use std::io::Read as _;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};

/// The exit code a shell reports for a program that does not exist.
const NOT_FOUND: i32 = 127;

/// The exit code a shell reports for a file that cannot be executed.
const NOT_EXECUTABLE: i32 = 126;

/// What a shell reports for a command killed by a signal, plus the signal.
const KILLED_BY_SIGNAL: i32 = 128;

/// How often a running gate is asked whether it has finished.
const POLL: Duration = Duration::from_millis(10);

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Exited(i32),
    TimedOut,
    /// It never started. The code is the one a shell would report.
    NotRun(i32),
}

/// Start the command, wait for it for at most `timeout`, and clean up.
///
/// The command runs from `directory` with no input, in a process group of its
/// own, so that when it is stopped everything it started is stopped with it.
/// Its standard error is passed through. Its standard output is returned when
/// `capture_stdout` is set, and otherwise goes to this process's standard
/// error, so that standard output can carry only a result.
///
/// # Errors
///
/// Returns the I/O error if waiting for the command failed.
///
/// # Panics
///
/// If `argv` is empty. A command has at least a program: the manifest schema
/// requires it, so an empty one is a defect in the caller.
pub fn execute(
    argv: &[String],
    directory: &Path,
    timeout: Duration,
    capture_stdout: bool,
    notify: impl Fn(&str),
) -> Result<(Ended, Option<Vec<u8>>), std::io::Error> {
    let (program, arguments) = argv
        .split_first()
        .expect("a command to have a program: the schema requires at least one element");

    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .process_group(0);
    if capture_stdout {
        command.stdout(Stdio::piped());
    } else {
        command.stdout(Stdio::from(std::io::stderr()));
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let code = if error.kind() == std::io::ErrorKind::NotFound {
                NOT_FOUND
            } else {
                NOT_EXECUTABLE
            };
            notify(&format!("{program}: {error}"));
            return Ok((Ended::NotRun(code), None));
        }
    };

    let reader = child.stdout.take().map(|mut standard_output| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = standard_output.read_to_end(&mut bytes);
            bytes
        })
    });

    let deadline = Instant::now() + timeout;
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ended::Exited(code_of(status)),
            Ok(None) => {}
            Err(error) => {
                stop_everything_started_by(&child);
                let _ = child.wait();
                return Err(error);
            }
        }
        if Instant::now() >= deadline {
            stop_everything_started_by(&child);
            let _ = child.wait();
            break Ended::TimedOut;
        }
        std::thread::sleep(POLL);
    };

    stop_everything_started_by(&child);
    let captured = reader.map(|reader| reader.join().unwrap_or_default());

    Ok((ended, captured))
}

/// The exit code of a command that has ended: its own, or the one a shell
/// reports for a command killed by a signal.
fn code_of(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| KILLED_BY_SIGNAL + signal))
        .unwrap_or(KILLED_BY_SIGNAL)
}

/// Send a kill to the command's process group, which holds the command and
/// everything it started. Failure means the group is already gone, which is
/// what was wanted.
fn stop_everything_started_by(child: &std::process::Child) {
    if let Some(group) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
        let _ = kill_process_group(group, Signal::KILL);
    }
}

/// What a caller of [`stream`] says after each line or tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Go {
    /// Carry on.
    On,
    /// Stop the command, and everything it started.
    Stop,
}

/// How a streamed command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Streamed {
    /// It exited with this code, or the one a shell reports for a signal.
    Exited(i32),
    /// It ran past its deadline and was stopped.
    TimedOut,
    /// The caller said to stop, and it was stopped.
    Stopped,
    /// It never started. The code is the one a shell would report.
    NotRun(i32),
}

/// Run a command and hand its standard output to the caller a line at a time,
/// as it is written.
///
/// The command runs from `directory` with no input, in a process group of its
/// own, with an environment that is cleared and then set to exactly
/// `environment`, so it holds nothing it was not given. Its standard error is
/// passed through. `on_line` is called with each line, and `on_tick` is called
/// every `tick` while nothing arrives, so a caller can send a heartbeat; each
/// says whether to carry on. When the deadline passes, or the caller says to
/// stop, or the command exits, everything the command started is killed. Lines
/// it had written before it exited are still delivered.
///
/// # Errors
///
/// Returns the I/O error if waiting for the command failed.
///
/// # Panics
///
/// If `argv` is empty. A command has at least a program: the manifest schema
/// requires it, so an empty one is a defect in the caller.
#[expect(
    clippy::too_many_arguments,
    reason = "each says one separate thing: what to run, where, with what, for how long, how often to tick, and the three things the caller does"
)]
pub fn stream(
    argv: &[String],
    directory: &Path,
    environment: &[(String, String)],
    deadline: Duration,
    tick: Duration,
    mut on_line: impl FnMut(&str) -> Go,
    mut on_tick: impl FnMut() -> Go,
    notify: impl Fn(&str),
) -> Result<Streamed, std::io::Error> {
    let (program, arguments) = argv
        .split_first()
        .expect("a command to have a program: the schema requires at least one element");

    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .process_group(0);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let code = if error.kind() == std::io::ErrorKind::NotFound {
                NOT_FOUND
            } else {
                NOT_EXECUTABLE
            };
            notify(&format!("{program}: {error}"));
            return Ok(Streamed::NotRun(code));
        }
    };

    let (sender, lines) = std::sync::mpsc::channel::<String>();
    let standard_output = child.stdout.take().expect("standard output was piped");
    let reader = std::thread::spawn(move || {
        for line in std::io::BufRead::lines(std::io::BufReader::new(standard_output)) {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let end_at = Instant::now() + deadline;
    let mut next_tick = Instant::now() + tick;
    let mut exit: Option<ExitStatus> = None;
    let ended = loop {
        if exit.is_none() {
            match child.try_wait() {
                Ok(status) => exit = status,
                Err(error) => {
                    stop_everything_started_by(&child);
                    let _ = child.wait();
                    return Err(error);
                }
            }
            if exit.is_some() {
                // Whatever it left running goes with it, and closes the pipe.
                stop_everything_started_by(&child);
            }
        }

        let now = Instant::now();
        if now >= end_at {
            // A command that has exited but whose output is still held open by
            // a process outside its group is not waited for beyond the
            // deadline: it did not run too long, and what it wrote has been
            // delivered.
            if let Some(status) = exit {
                break Streamed::Exited(code_of(status));
            }
            stop_everything_started_by(&child);
            let _ = child.wait();
            break Streamed::TimedOut;
        }

        let wait = next_tick
            .saturating_duration_since(now)
            .min(end_at.saturating_duration_since(now))
            .min(POLL);
        match lines.recv_timeout(wait) {
            Ok(line) => {
                if on_line(&line) == Go::Stop {
                    stop_everything_started_by(&child);
                    let _ = child.wait();
                    break Streamed::Stopped;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= next_tick {
                    next_tick = Instant::now() + tick;
                    if on_tick() == Go::Stop {
                        stop_everything_started_by(&child);
                        let _ = child.wait();
                        break Streamed::Stopped;
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Standard output is closed and everything written has been
                // delivered. The command is ending, or has.
                let status = match exit {
                    Some(status) => status,
                    None => child.wait()?,
                };
                break Streamed::Exited(code_of(status));
            }
        }
    };

    stop_everything_started_by(&child);
    // The reader is not joined. It ends when the pipe closes, which killing the
    // group brings about, except when a process outside the group holds the pipe
    // open, and then waiting for it would wait for that process.
    drop(reader);
    Ok(ended)
}
