//! Running a command for at most a given time, and stopping everything it
//! started.
//!
//! A command runs in a process group of its own. When its deadline passes, or
//! waiting for it fails, the whole group is killed, so a command cannot leave a
//! process behind that outlives it.

use std::collections::VecDeque;
use std::io::{BufRead as _, Read as _, Write as _};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};

/// Whether the process has been told to stop. A signal handler can only set a
/// flag, so every wait here looks at it each time it polls.
static CANCELLED: LazyLock<Arc<AtomicBool>> = LazyLock::new(|| Arc::new(AtomicBool::new(false)));

/// Make an interrupt, a termination or a hang-up set the cancellation flag
/// instead of ending the process, so that what it is running can be stopped
/// properly. Call it once, before starting any thread of your own.
///
/// # Errors
///
/// Returns the I/O error if a handler could not be installed.
pub fn install_cancellation() -> std::io::Result<()> {
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&CANCELLED))?;
    }
    Ok(())
}

/// Set the cancellation flag, as a signal does.
pub fn cancel() {
    CANCELLED.store(true, Ordering::SeqCst);
}

/// Whether the process has been told to stop.
#[must_use]
pub fn cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}

/// Clear the cancellation flag. For tests, which share one process.
#[doc(hidden)]
pub fn reset_cancellation() {
    CANCELLED.store(false, Ordering::SeqCst);
}

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
    /// The process was told to stop, and the command was killed with everything it
    /// started, or was not started.
    Cancelled,
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
    execute_with_env(argv, directory, timeout, capture_stdout, &[], notify)
}

/// The last lines a command printed, kept while it runs so that a caller can say what a command
/// that failed had said last. It is shared: clones see the same lines.
#[derive(Debug, Clone)]
pub struct Tail {
    lines: Arc<Mutex<Kept>>,
    most: usize,
}

/// The lines held, and how many were printed in all.
#[derive(Debug, Default)]
struct Kept {
    lines: VecDeque<String>,
    printed: usize,
}

impl Tail {
    /// A tail that keeps the last `most` lines.
    #[must_use]
    pub fn new(most: usize) -> Self {
        Self {
            lines: Arc::new(Mutex::new(Kept::default())),
            most,
        }
    }

    fn push(&self, line: String) {
        if let Ok(mut kept) = self.lines.lock() {
            kept.printed += 1;
            kept.lines.push_back(line);
            while kept.lines.len() > self.most {
                kept.lines.pop_front();
            }
        }
    }

    /// The lines kept, oldest first, joined with newlines.
    #[must_use]
    pub fn text(&self) -> String {
        self.lines.lock().map_or_else(
            |_| String::new(),
            |kept| {
                kept.lines
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        )
    }

    /// The last `most` lines kept, joined with newlines, and how many lines printed before them
    /// that are not among them.
    #[must_use]
    pub fn last(&self, most: usize) -> (String, usize) {
        self.lines.lock().map_or_else(
            |_| (String::new(), 0),
            |kept| {
                let shown = kept.lines.len().min(most);
                let text = kept
                    .lines
                    .iter()
                    .skip(kept.lines.len() - shown)
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
                (text, kept.printed - shown)
            },
        )
    }
}

/// Copy what a command prints, a line at a time, to this process's standard error, as the
/// command would have printed it there itself, and into the tail.
fn tee(output: impl std::io::Read + Send + 'static, tail: Tail) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(output).split(b'\n') {
            let Ok(bytes) = line else { break };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let _ = writeln!(std::io::stderr(), "{text}");
            tail.push(text);
        }
    })
}

/// [`execute`], with these variables set in the command's environment on top of
/// this process's own.
///
/// # Errors
///
/// Returns the I/O error if waiting for the command failed.
///
/// # Panics
///
/// If `argv` is empty, as for [`execute`].
pub fn execute_with_env(
    argv: &[String],
    directory: &Path,
    timeout: Duration,
    capture_stdout: bool,
    environment: &[(String, String)],
    notify: impl Fn(&str),
) -> Result<(Ended, Option<Vec<u8>>), std::io::Error> {
    execute_with_tail(
        argv,
        directory,
        timeout,
        capture_stdout,
        environment,
        notify,
        None,
        None,
    )
}

/// What a caller does every so often while it waits for a command that may take long, so that
/// whoever watches it can tell it is still alive. `call` says whether to carry on: when it says
/// to stop, the command is killed and the wait ends as cancelled.
pub struct Tick<'a> {
    pub every: Duration,
    pub call: &'a mut dyn FnMut() -> Go,
}

impl Tick<'_> {
    /// A tick that calls the same thing at the same interval, to hand to one wait while this one
    /// is kept for the next.
    pub fn reborrow(&mut self) -> Tick<'_> {
        Tick {
            every: self.every,
            call: &mut *self.call,
        }
    }
}

/// [`execute_with_env`], keeping the last lines the command printed in `tail`. What it prints
/// still goes to this process's standard error, a line at a time, as it would have; its
/// standard output, when it is not the one captured, goes there too. `tick`, when there is one,
/// is called every `tick.every` for as long as the command runs, and never after it ends.
///
/// # Errors
///
/// As for [`execute_with_env`].
///
/// # Panics
///
/// If `argv` is empty, as for [`execute`].
#[expect(
    clippy::too_many_arguments,
    reason = "each says one separate thing: what to run, where, for how long, what to capture, with what, and the three things the caller does while it runs"
)]
pub fn execute_with_tail(
    argv: &[String],
    directory: &Path,
    timeout: Duration,
    capture_stdout: bool,
    environment: &[(String, String)],
    notify: impl Fn(&str),
    tail: Option<&Tail>,
    mut tick: Option<Tick<'_>>,
) -> Result<(Ended, Option<Vec<u8>>), std::io::Error> {
    let (program, arguments) = argv
        .split_first()
        .expect("a command to have a program: the schema requires at least one element");

    if cancelled() {
        return Ok((Ended::Cancelled, None));
    }

    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(directory)
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .process_group(0);
    // What it prints is kept in a tail when there is one, and is otherwise left to go to this
    // process's standard error itself.
    if tail.is_some() {
        command.stderr(Stdio::piped());
    } else {
        command.stderr(Stdio::inherit());
    }
    if capture_stdout || tail.is_some() {
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

    let mut tees = Vec::new();
    if let Some(tail) = tail {
        if let Some(standard_error) = child.stderr.take() {
            tees.push(tee(standard_error, tail.clone()));
        }
        if !capture_stdout && let Some(standard_output) = child.stdout.take() {
            tees.push(tee(standard_output, tail.clone()));
        }
    }
    let reader = child.stdout.take().map(|mut standard_output| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = standard_output.read_to_end(&mut bytes);
            bytes
        })
    });

    let deadline = Instant::now() + timeout;
    let mut next_tick = tick.as_ref().map(|tick| Instant::now() + tick.every);
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
        if cancelled() {
            stop_everything_started_by(&child);
            let _ = child.wait();
            break Ended::Cancelled;
        }
        if Instant::now() >= deadline {
            stop_everything_started_by(&child);
            let _ = child.wait();
            break Ended::TimedOut;
        }
        if let (Some(tick), Some(due)) = (tick.as_mut(), next_tick)
            && Instant::now() >= due
        {
            next_tick = Some(Instant::now() + tick.every);
            if (tick.call)() == Go::Stop {
                stop_everything_started_by(&child);
                let _ = child.wait();
                break Ended::Cancelled;
            }
        }
        std::thread::sleep(POLL);
    };

    stop_everything_started_by(&child);
    let captured = reader.map(|reader| reader.join().unwrap_or_default());
    for tee in tees {
        let _ = tee.join();
    }

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

/// Read a command's output a line at a time on a thread of its own, so that
/// whoever waits for it can also notice other things.
fn read_lines(
    output: impl std::io::Read + Send + 'static,
) -> (
    std::sync::mpsc::Receiver<String>,
    std::thread::JoinHandle<()>,
) {
    let (sender, lines) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufRead::lines(std::io::BufReader::new(output)) {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    (lines, reader)
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
    /// The process was told to stop, and the command was killed with everything it
    /// started, or was not started.
    Cancelled,
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
    on_line: impl FnMut(&str) -> Go,
    on_tick: impl FnMut() -> Go,
    notify: impl Fn(&str),
) -> Result<Streamed, std::io::Error> {
    stream_with_tail(
        argv,
        directory,
        environment,
        deadline,
        tick,
        on_line,
        on_tick,
        notify,
        None,
    )
}

/// [`stream`], keeping the last lines the command wrote to standard error in `tail`. They still
/// go to this process's standard error a line at a time, as they would have. A caller that
/// ends a run on the command's failure uses them to say why, since the exit code alone does not.
///
/// # Errors
///
/// As for [`stream`].
///
/// # Panics
///
/// If `argv` is empty, as for [`stream`].
#[expect(
    clippy::too_many_arguments,
    reason = "each says one separate thing: what to run, where, with what, for how long, how often to tick, the three things the caller does, and what to keep"
)]
pub fn stream_with_tail(
    argv: &[String],
    directory: &Path,
    environment: &[(String, String)],
    deadline: Duration,
    tick: Duration,
    mut on_line: impl FnMut(&str) -> Go,
    mut on_tick: impl FnMut() -> Go,
    notify: impl Fn(&str),
    tail: Option<&Tail>,
) -> Result<Streamed, std::io::Error> {
    let (program, arguments) = argv
        .split_first()
        .expect("a command to have a program: the schema requires at least one element");

    if cancelled() {
        return Ok(Streamed::Cancelled);
    }

    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if tail.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .process_group(0);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            notify(&format!("{program}: {error}"));
            return Ok(Streamed::NotRun(code_when_not_started(&error)));
        }
    };

    let standard_output = child.stdout.take().expect("standard output was piped");
    let (lines, reader) = read_lines(standard_output);
    let tee = tail.and_then(|tail| child.stderr.take().map(|output| tee(output, tail.clone())));

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

        if cancelled() {
            stop_everything_started_by(&child);
            let _ = child.wait();
            break Streamed::Cancelled;
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
    give_a_moment_to_finish(tee);
    Ok(ended)
}

/// What a command wrote last is what explains a failure, so its copying is given a moment to
/// finish; but not waited for beyond that, because a process outside the command's group can
/// hold the pipe open.
fn give_a_moment_to_finish(copying: Option<std::thread::JoinHandle<()>>) {
    if let Some(copying) = copying {
        let until = Instant::now() + Duration::from_secs(2);
        while !copying.is_finished() && Instant::now() < until {
            std::thread::sleep(POLL);
        }
    }
}

/// The code a shell reports for a command that could not be started.
fn code_when_not_started(error: &std::io::Error) -> i32 {
    if error.kind() == std::io::ErrorKind::NotFound {
        NOT_FOUND
    } else {
        NOT_EXECUTABLE
    }
}

/// A process found running with its working directory under a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub pid: i32,
    /// Its working directory, with symbolic links resolved.
    pub directory: PathBuf,
}

/// The processes, other than this one, whose working directory is under `directory`.
///
/// A process that left its command's process group, as a shell with job control or `setsid`
/// does, is not reached by stopping the group, but it still works in the directory it was
/// started in. On Linux this reads `/proc`; elsewhere it asks `lsof`. When neither can be read
/// nothing is found, which is the same as nothing being running, so a caller must not take
/// an empty answer as proof.
#[must_use]
pub fn processes_under(directory: &Path) -> Vec<Found> {
    let Ok(root) = directory.canonicalize() else {
        return Vec::new();
    };
    let own = i32::try_from(std::process::id()).unwrap_or(0);
    let mut found: Vec<Found> = working_directories()
        .into_iter()
        .filter(|found| found.pid != own && found.directory.starts_with(&root))
        .collect();
    found.sort_by_key(|found| found.pid);
    found
}

fn working_directories() -> Vec<Found> {
    if Path::new("/proc/self/cwd").exists() {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        return entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
                let directory = std::fs::read_link(entry.path().join("cwd")).ok()?;
                Some(Found { pid, directory })
            })
            .collect();
    }
    let Ok(output) = Command::new("lsof")
        .args(["-a", "-d", "cwd", "-F", "pn"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut pid = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(number) = line.strip_prefix('p') {
            pid = number.parse::<i32>().ok();
        } else if let (Some(name), Some(pid)) = (line.strip_prefix('n'), pid) {
            found.push(Found {
                pid,
                directory: PathBuf::from(name),
            });
        }
    }
    found
}

/// Kill one process. Failure means it is already gone, which is what was wanted.
pub fn kill_found(found: &Found) {
    if let Some(pid) = Pid::from_raw(found.pid) {
        let _ = rustix::process::kill_process(pid, Signal::KILL);
    }
}

/// Kill every process working under `directory`, and look again until none is left, since a
/// loop that was killed may have started another a moment before. Returns every process killed.
#[must_use]
pub fn stop_processes_under(directory: &Path) -> Vec<Found> {
    let mut stopped = Vec::new();
    for _ in 0..10 {
        let found = processes_under(directory);
        if found.is_empty() {
            break;
        }
        for process in &found {
            kill_found(process);
        }
        stopped.extend(found);
        std::thread::sleep(POLL * 5);
    }
    stopped
}

/// The machine's load average over one, five and fifteen minutes, as it prints them, so that a
/// timeout can say whether the machine was busy.
#[must_use]
pub fn load_average() -> Option<String> {
    let text = std::fs::read_to_string("/proc/loadavg").ok().or_else(|| {
        let output = Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        String::from_utf8(output.stdout).ok()
    })?;
    let numbers: Vec<&str> = text
        .split_whitespace()
        .filter(|word| word.parse::<f64>().is_ok())
        .take(3)
        .collect();
    (numbers.len() == 3).then(|| numbers.join(" "))
}
