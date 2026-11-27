//! `heph worker`: do the work a hosted control plane gives this machine.
//!
//! The worker opens every connection. It asks the control plane for a workload, runs the
//! executor on it as `heph run` does, sends the executor's events in order as they come, keeps
//! the lease alive, and sends the run record at the end. Model keys, the code host's token and
//! the repository's own scripts stay on this machine: the worker reads the executor's secrets
//! only to hand the executor the files they are in, and sends nothing but what the executor
//! wrote, which has already had every secret it holds taken out.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher as _, Hasher as _};
use std::io::{BufRead as _, BufReader};
use std::os::unix::fs::DirBuilderExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};
use serde_json::Value;

use crate::link::{Claim, Client, Failure, Token};
use crate::run;
use crate::run_lock;
use crate::workspaces;

/// The variable the device token is given in.
pub const TOKEN_VARIABLE: &str = "HEPH_WORKER_TOKEN";

/// The variable that names a file the device token is in.
pub const TOKEN_FILE_VARIABLE: &str = "HEPH_WORKER_TOKEN_FILE";

/// Where the token is looked for, under the configuration directory, when no variable says.
const TOKEN_FILE: &str = "worker-token";

/// The first wait after a failure to reach the control plane, which doubles each time.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);

/// The longest wait between tries.
const BACKOFF_LONGEST: Duration = Duration::from_secs(60);

/// The longest wait between tries of a request that belongs to a run in progress, which must be
/// made again well inside the lease.
const BACKOFF_IN_A_RUN: Duration = Duration::from_secs(5);

/// A claim that is answered with none sooner than this after it was asked is not a long poll
/// that waited, so the next one is not made at once.
const SHORTEST_POLL: Duration = Duration::from_secs(1);

/// How long an executor that has been told to stop is given before it is killed.
const GRACE: Duration = Duration::from_secs(10);

/// The most events sent in one request.
const MOST_EVENTS: usize = 500;

/// How often what is waited for is looked at: a stop, a line from the executor.
const LOOK: Duration = Duration::from_millis(100);

/// The variable that sets how many workloads this worker runs at once, when no flag does.
pub const WORKLOADS_VARIABLE: &str = "HEPH_WORKER_WORKLOADS";

/// How many workloads a worker runs at once when nothing says.
pub const DEFAULT_WORKLOADS: usize = 1;

/// The most workloads a worker may be asked to run at once.
///
/// This is a guard against a typing mistake, so that `--workloads 640` does not start
/// hundreds of model sessions and workspaces. It is not a measured limit, and nothing was
/// measured to choose it: the right number depends on the machine and is the person's to
/// choose. What each slot costs is one workspace on disk and one model session, and in
/// practice the worker's memory and the model's rate limit are what bound it.
pub const MOST_WORKLOADS: usize = 64;

/// What `run` was asked to do.
pub struct Options {
    pub control_plane: String,
    pub runs_dir: Option<PathBuf>,
    pub executor: Option<PathBuf>,
    /// How many workloads may run at once, from 1 to [`MOST_WORKLOADS`].
    pub workloads: usize,
    /// How long a run's workspace is kept after the run was last written.
    pub keep_runs_for: Duration,
}

/// The variable that sets how many days a finished run keeps its workspace, when no flag does.
pub const KEEP_RUNS_VARIABLE: &str = "HEPH_WORKER_KEEP_RUNS";

/// How many days a finished run keeps its workspace when nothing says.
pub const DEFAULT_KEEP_DAYS: u64 = 7;

/// How often the worker looks for workspaces to clear.
const CLEAR_EVERY: Duration = Duration::from_secs(3600);

/// How long finished runs keep their workspaces: the flag's number of days, else the
/// variable's, else seven.
///
/// # Errors
///
/// Returns why the setting is refused: it is not a whole number of days.
pub fn keep_runs_for(flag: Option<&str>) -> Result<Duration, String> {
    keep_runs_from(flag, run::environment(KEEP_RUNS_VARIABLE).as_deref())
}

fn keep_runs_from(flag: Option<&str>, variable: Option<&str>) -> Result<Duration, String> {
    let (text, from) = match (flag, variable) {
        (Some(text), _) => (text, "--keep-runs-for".to_owned()),
        (None, Some(text)) => (text, KEEP_RUNS_VARIABLE.to_owned()),
        (None, None) => return Ok(Duration::from_secs(DEFAULT_KEEP_DAYS * 86_400)),
    };
    let days: u64 = text
        .trim()
        .parse()
        .map_err(|_| format!("{from}: {text:?} is not a whole number of days"))?;
    Ok(Duration::from_secs(days.saturating_mul(86_400)))
}

/// Remove the workspaces of runs that were last written more than `keep` ago, except those
/// the control plane says it still needs. When it cannot say, nothing is removed.
fn clear_old_workspaces(client: &Client, runs: &Path, keep: Duration) {
    let old = workspaces::old_checkouts(runs, keep, std::time::SystemTime::now());
    if old.is_empty() {
        return;
    }
    let ids: Vec<String> = old.iter().map(|run| run.id.clone()).collect();
    let held = match client.held(&ids) {
        Ok(held) => held,
        Err(failure) => {
            eprintln!(
                "heph: worker: could not ask which workspaces are still needed ({failure}); none was removed"
            );
            return;
        }
    };
    let due: Vec<workspaces::Old> = old
        .into_iter()
        .filter(|run| !held.contains(&run.id))
        .collect();
    let cleared = workspaces::clear(&due);
    for warning in &cleared.warnings {
        eprintln!("heph: warning: {warning}");
    }
    if cleared.removed > 0 {
        println!(
            "heph: worker: removed {} workspace{} of runs finished more than {} days ago, and freed {}",
            cleared.removed,
            if cleared.removed == 1 { "" } else { "s" },
            keep.as_secs() / 86_400,
            workspaces::show_size(cleared.freed)
        );
    }
}

/// How many workloads to run at once: the flag's value, else the variable's, else one.
///
/// # Errors
///
/// Returns why the number is refused: it is not a whole number, it is zero, or it is above
/// [`MOST_WORKLOADS`].
pub fn workloads(flag: Option<&str>) -> Result<usize, String> {
    workloads_from(flag, run::environment(WORKLOADS_VARIABLE).as_deref())
}

fn workloads_from(flag: Option<&str>, variable: Option<&str>) -> Result<usize, String> {
    let (text, from) = match (flag, variable) {
        (Some(text), _) => (text, "--workloads".to_owned()),
        (None, Some(text)) => (text, WORKLOADS_VARIABLE.to_owned()),
        (None, None) => return Ok(DEFAULT_WORKLOADS),
    };
    let number: usize = text.trim().parse().map_err(|_| {
        format!("{from}: {text:?} is not a whole number of workloads, from 1 to {MOST_WORKLOADS}")
    })?;
    check_workloads(number).map_err(|message| format!("{from}: {message}"))
}

fn check_workloads(number: usize) -> Result<usize, String> {
    if number == 0 {
        Err(
            "a worker must be able to run at least one workload at once, so 0 is refused"
                .to_owned(),
        )
    } else if number > MOST_WORKLOADS {
        Err(format!(
            "{number} is above the most a worker may run at once, {MOST_WORKLOADS}: each one holds a workspace on disk and a model session"
        ))
    } else {
        Ok(number)
    }
}

/// The device token: from `HEPH_WORKER_TOKEN`, else the file `HEPH_WORKER_TOKEN_FILE` names,
/// else `worker-token` under the configuration directory. A file must be one only its owner
/// can read. It is never taken from an argument, which a process listing would show, and no
/// message made here holds it.
///
/// # Errors
///
/// Returns where to put the token when there is none, or what is wrong with the one given.
pub fn token() -> Result<Token, String> {
    token_from(
        run::environment(TOKEN_VARIABLE),
        run::environment(TOKEN_FILE_VARIABLE),
        run::config_dir().map(|dir| dir.join(TOKEN_FILE)),
    )
}

fn token_from(
    variable: Option<String>,
    named_file: Option<String>,
    default_file: Option<PathBuf>,
) -> Result<Token, String> {
    match (variable, named_file) {
        (Some(_), Some(_)) => Err(format!(
            "{TOKEN_VARIABLE} and {TOKEN_FILE_VARIABLE} are both set: give one"
        )),
        (Some(value), None) => Ok(Token::new(value.trim().to_owned())),
        (None, Some(path)) => read_token(Path::new(&path), TOKEN_FILE_VARIABLE),
        (None, None) => match default_file.clone().filter(|path| path.is_file()) {
            Some(path) => read_token(&path, "the token file"),
            None => Err(format!(
                "no device token: set {TOKEN_VARIABLE}, or {TOKEN_FILE_VARIABLE} to a file only you can read, or put the token in {} (mode 600). A signed-in person makes one in the control plane",
                default_file.map_or_else(
                    || format!("~/.config/hephaestium/{TOKEN_FILE}"),
                    |path| path.display().to_string()
                )
            )),
        },
    }
}

fn read_token(path: &Path, what: &str) -> Result<Token, String> {
    let shown = path.display();
    let metadata = std::fs::metadata(path).map_err(|error| format!("{what}: {shown}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{what}: {shown} is not a regular file"));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{what}: {shown} can be read by others (mode {:03o}); run: chmod 600 {shown}",
            metadata.mode() & 0o777
        ));
    }
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{what}: {shown}: {error}"))?;
    let value = text.trim();
    if value.is_empty() {
        return Err(format!("{what}: {shown} is empty"));
    }
    Ok(Token::new(value.to_owned()))
}

/// How long to wait after the `failures`th failure in a row to do something: it doubles from
/// `first` up to `longest`, and is made random between half of that and all of it, so that
/// machines that lost the same connection together do not all come back together.
fn backoff(failures: u32, first: Duration, longest: Duration) -> Duration {
    let doubled = first.saturating_mul(2_u32.saturating_pow(failures.saturating_sub(1)));
    let wait = doubled.min(longest);
    // A random number from the standard library's own seeded hasher: nothing here needs more.
    let random = RandomState::new().build_hasher().finish();
    #[expect(
        clippy::cast_precision_loss,
        reason = "a fraction of a wait does not need all 64 bits"
    )]
    let fraction = (random >> 11) as f64 / (1_u64 << 53) as f64;
    wait.mul_f64(0.5 + fraction / 2.0)
}

/// Wait, looking for a stop. False when there was one.
fn pause(wait: Duration, stop: &dyn Fn() -> bool) -> bool {
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if stop() {
            return false;
        }
        std::thread::sleep(LOOK.min(until.saturating_duration_since(Instant::now())));
    }
    !stop()
}

/// What a claim that was made on a thread of its own came to.
enum Asked {
    Answer(Result<Option<Claim>, Failure>),
    Stopped,
}

/// Ask for a workload without being held by the wait when a stop comes. A claim that is
/// answered after the stop is dropped, and the control plane's lease on it lapses.
fn ask(client: &Client, slots: usize, stop: &dyn Fn() -> bool) -> Asked {
    let (sender, answer) = mpsc::channel();
    let client = client.clone();
    std::thread::spawn(move || {
        let _ = sender.send(client.claim(slots));
    });
    loop {
        if stop() {
            return Asked::Stopped;
        }
        match answer.recv_timeout(LOOK) {
            Ok(answer) => return Asked::Answer(answer),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Asked::Answer(Err(Failure::Trouble(
                    "the claim ended without an answer".to_owned(),
                )));
            }
        }
    }
}

/// Claim workloads and run them until told to stop.
///
/// A worker has as many slots as it may run workloads at once. Each slot claims on its own,
/// runs what it is given in a workspace of its own, and holds a lease of its own, so one
/// finishing, failing or being stopped does not touch another. A stop reaches every slot, and
/// each gives back what it holds. A claim whose answer is lost is never run: the worker never
/// saw it, and the control plane's lease on it lapses.
///
/// # Errors
///
/// Returns why the worker cannot go on: the number of workloads is refused, the address is
/// unusable, the executor's secrets are missing, or the control plane does not accept the token.
pub fn run(
    options: &Options,
    token: Token,
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<(), String> {
    check_workloads(options.workloads)?;
    let client = Client::new(&options.control_plane, token)?;
    // Found out before anything is claimed, so that a worker that could not deliver does not
    // take work it will only fail.
    run::secret_files()?;

    // Set when one slot finds the worker cannot go on, which ends the others too.
    let halted = AtomicBool::new(false);
    let stopping = || halted.load(Ordering::Relaxed) || stop();
    let slots = options.workloads;
    let runs = run::runs_dir(options.runs_dir.as_deref())?;
    let results: Vec<Result<(), String>> = std::thread::scope(|scope| {
        // Clearing never ends the worker: what it cannot do is said and tried again later.
        let clearing = scope.spawn(|| {
            loop {
                clear_old_workspaces(&client, &runs, options.keep_runs_for);
                if !pause(CLEAR_EVERY, &stopping) {
                    break;
                }
            }
            Ok(())
        });
        let handles: Vec<_> = (0..slots)
            .map(|_| {
                scope.spawn(|| {
                    let result = slot(&client, options, slots, &stopping);
                    if result.is_err() {
                        halted.store(true, Ordering::Relaxed);
                    }
                    result
                })
            })
            .collect();
        let mut handles = handles;
        handles.push(clearing);
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err("a workload slot stopped unexpectedly".to_owned()))
            })
            .collect()
    });
    results.into_iter().collect::<Result<Vec<()>, String>>()?;
    Ok(())
}

/// Report and stop the processes whose working directory is under a run directory that nobody
/// holds: a run that has finished, however it ended, or whose worker died, started nothing that
/// should still be running. The executor stops its own run's processes; this catches those that
/// escaped it, or that an executor which was itself killed never got to stop. A directory held by
/// any process, this worker or another on the machine, is a live run and is left alone.
fn stop_what_finished_runs_left(options: &Options) {
    stop_what_finished_runs_left_logging(options, &mut std::io::stderr());
}

/// As `stop_what_finished_runs_left`, saying each stop to `log`.
fn stop_what_finished_runs_left_logging(options: &Options, log: &mut dyn std::io::Write) {
    let Ok(runs) = run::runs_dir(options.runs_dir.as_deref()) else {
        return;
    };
    let Ok(root) = runs.canonicalize() else {
        return;
    };
    for found in crate::process::processes_under(&root) {
        // A run directory is `<runs>/<repository>/<ticket>-<id>`.
        let run: PathBuf = found
            .directory
            .strip_prefix(&root)
            .map(|inside| inside.components().take(2).collect())
            .unwrap_or_default();
        let run = root.join(run);
        if crate::run_lock::is_held(&run) {
            continue;
        }
        // A log that cannot be written (a closed stderr, a full pipe) must not
        // stop the sweep from stopping the process, so the error is ignored.
        let _ = writeln!(
            log,
            "heph: worker: stopping process {}, left running in {}: nobody holds the run {}, so it is taken for finished",
            found.pid,
            found.directory.display(),
            run.display()
        );
        crate::process::kill_found(&found);
    }
}

/// One slot: claim a workload, run it, and ask again, until told to stop.
fn slot(
    client: &Client,
    options: &Options,
    slots: usize,
    stop: &dyn Fn() -> bool,
) -> Result<(), String> {
    let mut failures: u32 = 0;
    while !stop() {
        stop_what_finished_runs_left(options);
        let asked_at = Instant::now();
        let claim = match ask(client, slots, stop) {
            Asked::Stopped => break,
            Asked::Answer(Ok(claim)) => {
                failures = 0;
                claim
            }
            Asked::Answer(Err(Failure::Denied)) => {
                return Err(
                    "the control plane does not accept the device token: it was revoked or is not one it made"
                        .to_owned(),
                );
            }
            Asked::Answer(Err(failure)) => {
                failures = failures.saturating_add(1);
                let wait = backoff(failures, BACKOFF_FIRST, BACKOFF_LONGEST);
                eprintln!(
                    "heph: worker: could not claim ({failure}); trying again in {} seconds",
                    wait.as_secs().max(1)
                );
                pause(wait, stop);
                continue;
            }
        };
        let Some(claim) = claim else {
            if asked_at.elapsed() < SHORTEST_POLL {
                pause(SHORTEST_POLL, stop);
            }
            continue;
        };
        // A stop that came while the answer was on its way: nothing is started.
        if stop() {
            break;
        }
        if let Err(message) = serve(client, &claim, options, stop) {
            eprintln!("heph: worker: workload {}: {message}", claim.id);
        }
    }
    Ok(())
}

/// A name that is safe to make a directory of.
fn safe(text: &str) -> Option<&str> {
    let ok = !text.is_empty()
        && text != "."
        && text != ".."
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    ok.then_some(text)
}

/// Make the directory a run is kept in, outside any project, and put the workload in it.
fn make_run_directory(claim: &Claim, options: &Options) -> Result<PathBuf, String> {
    let workload = &claim.workload;
    let id = safe(&claim.id).ok_or("its id is not one that can name a directory")?;
    let repository = format!(
        "{}-{}",
        safe(workload["repository"]["owner"].as_str().unwrap_or_default())
            .ok_or("it names no repository owner")?,
        safe(workload["repository"]["name"].as_str().unwrap_or_default())
            .ok_or("it names no repository")?
    );
    let ticket = safe(workload["ticket"]["number"].as_str().unwrap_or("0")).unwrap_or("0");

    let keep = run::keep_window(None)?;
    let runs = run::runs_dir(options.runs_dir.as_deref())?;
    for warning in &workspaces::sweep(&runs, keep, std::time::SystemTime::now()).warnings {
        eprintln!("heph: warning: {warning}");
    }
    let directory = runs.join(repository).join(format!("{ticket}-{id}"));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)
        .map_err(|e| format!("could not make {}: {e}", directory.display()))?;
    let text = serde_json::to_string_pretty(workload).map_err(|e| e.to_string())?;
    let workload_file = directory.join("workload.json");
    std::fs::write(&workload_file, text + "\n")
        .map_err(|e| format!("could not write {}: {e}", workload_file.display()))?;
    Ok(directory)
}

/// Run one claimed workload and send what it says. The error is why it could not be run at all;
/// the control plane's lease on it then lapses.
fn serve(
    client: &Client,
    claim: &Claim,
    options: &Options,
    stop: &dyn Fn() -> bool,
) -> Result<(), String> {
    let directory = make_run_directory(claim, options)?;
    // Held until this run ends, so that no sweep takes its processes for a finished run's.
    let _held = run_lock::hold(&directory)?;
    serve_in(client, claim, options, stop, &directory)
}

fn serve_in(
    client: &Client,
    claim: &Claim,
    options: &Options,
    stop: &dyn Fn() -> bool,
    directory: &Path,
) -> Result<(), String> {
    let workload_file = directory.join("workload.json");
    let offered = run::secret_files()?;
    let work = directory.join("work");
    let record = directory.join("record.json");
    // A workload that was put back is run again from the start: what a lost run left is of no
    // use, and its record must not be taken for this run's.
    let _ = std::fs::remove_file(&record);
    let _ = std::fs::remove_dir_all(&work);
    let log = std::fs::File::create(directory.join("executor.log"))
        .map_err(|e| format!("could not make the executor's log: {e}"))?;

    let this_program = std::env::current_exe().ok();
    let executor = run::executor_path(
        options.executor.as_deref(),
        this_program.as_deref().and_then(Path::parent),
    );
    let mut command = run::executor_command(&executor, &workload_file, &work, &record, offered);
    // The executor, and the agent and the scripts under it, are not given the token that
    // speaks for this machine to the control plane.
    command
        .env_remove(TOKEN_VARIABLE)
        .env_remove(TOKEN_FILE_VARIABLE)
        .stderr(Stdio::from(log));
    let mut child = command.spawn().map_err(|error| {
        format!(
            "could not start the executor ({}): {error}",
            executor.display()
        )
    })?;
    println!("workload {}: running", claim.id);

    let output = child.stdout.take().ok_or("the executor has no output")?;
    let lines = read_lines(output);
    let mut sending = Sending {
        client,
        claim,
        last_ok: Instant::now(),
        started: Instant::now(),
    };
    let followed = sending.follow(&mut child, &lines, stop);
    let gone = followed.gone;
    let status = child
        .wait()
        .map_err(|e| format!("waiting for the executor failed: {e}"))?;
    if let Some(why) = gone {
        println!(
            "workload {}: stopped, because {why}; nothing more is sent for it",
            claim.id
        );
        return Ok(());
    }

    let text = std::fs::read_to_string(&record);
    if followed.stopped_by_operator && !says_it_finished(text.as_deref().ok()) {
        // The operator stopped this machine's executor, so the work did not finish and the
        // ticket did not change. What the executor said while it stopped ("cancelled") is not
        // sent, because the control plane would end the workload for good on it.
        sending.give_back();
        return Ok(());
    }
    // A stop that came too late to matter: the run ended as it would have, and what was held
    // back while it stopped is sent after all.
    if !followed.held.is_empty() {
        let held: Vec<String> = followed
            .held
            .iter()
            .map(|line| sending.numbered(line))
            .collect();
        if let Delivered::Gone(why) = sending.deliver(|| client.events(&claim.id, &held)) {
            println!(
                "workload {}: the events were not kept, because {why}",
                claim.id
            );
            return Ok(());
        }
    }
    let Ok(text) = text else {
        // Nothing is made up in its place: the control plane sees the lease lapse and decides
        // from the events what that means.
        return Err(format!(
            "the executor ended ({}) without a run record: {}",
            status
                .code()
                .map_or_else(|| "by a signal".to_owned(), |c| format!("code {c}")),
            tail(&directory.join("executor.log"))
        ));
    };
    let value: Value = serde_json::from_str(&text)
        .ok()
        .filter(Value::is_object)
        .ok_or("the run record is not a JSON object")?;
    match sending.deliver(|| client.record(&claim.id, &text)) {
        Delivered::Yes => {}
        Delivered::Gone(why) => {
            println!(
                "workload {}: the record was not kept, because {why}",
                claim.id
            );
            return Ok(());
        }
    }
    let outcome = value["outcome"]
        .as_str()
        .unwrap_or("OUTCOME_UNSPECIFIED")
        .trim_start_matches("OUTCOME_")
        .to_lowercase();
    println!("workload {}: {outcome}", claim.id);
    if outcome == "completed"
        && let Err(warning) = workspaces::delete_checkout(directory)
    {
        eprintln!("heph: warning: {warning}");
    }
    discard_build_output_of_an_unfinished_run(&outcome, directory);
    Ok(())
}

/// A run that ended without completing keeps its source to be looked at, and drops its
/// build output, which is most of the disk and which nobody reads.
fn discard_build_output_of_an_unfinished_run(outcome: &str, directory: &Path) {
    if outcome != "completed"
        && outcome != "unspecified"
        && let Err(warning) = workspaces::discard_build_output(directory)
    {
        eprintln!("heph: warning: {warning}");
    }
}

/// Whether a run record says the run ended by itself, as any outcome but a cancellation.
fn says_it_finished(record: Option<&str>) -> bool {
    let outcome = record
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| value["outcome"].as_str().map(str::to_owned));
    outcome.is_some_and(|outcome| outcome != "OUTCOME_CANCELLED")
}

/// The executor's output a line at a time, on a thread of its own so that this one can notice a
/// stop, the end of a lease and the time for a heartbeat while the executor is quiet.
fn read_lines(output: std::process::ChildStdout) -> Receiver<String> {
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).split(b'\n') {
            let Ok(bytes) = line else { break };
            if sender
                .send(String::from_utf8_lossy(&bytes).into_owned())
                .is_err()
            {
                break;
            }
        }
    });
    lines
}

/// The last few lines of a file, for saying what went wrong.
fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(8)..].join("\n")
}

/// How the following of one run ended.
struct Followed {
    /// Why the run was stopped when the control plane took the workload back.
    gone: Option<String>,
    /// Whether this program was told to stop, and so told the executor to.
    stopped_by_operator: bool,
    /// What the executor said after it was told to stop, not yet sent.
    held: Vec<String>,
}

enum Delivered {
    Yes,
    /// The control plane does not hold the workload for this machine any more, or will not
    /// take what is sent, or could not be reached for as long as a lease lasts. Why.
    Gone(String),
}

/// What is sent for one run, and how long it has been since anything was.
struct Sending<'a> {
    client: &'a Client,
    claim: &'a Claim,
    last_ok: Instant,
    started: Instant,
}

impl Sending<'_> {
    /// Read the executor's output until it ends, sending each event as it comes. A stop asked
    /// of this program is passed on once, to the executor, which says how it ended; and
    /// reading goes on until it has. What it says after that is held back and not sent, because
    /// a cancelled run would end the workload for good, when the operator's stop is only a
    /// reason to give the work back.
    fn follow(
        &mut self,
        child: &mut Child,
        lines: &Receiver<String>,
        stop: &dyn Fn() -> bool,
    ) -> Followed {
        let (client, claim) = (self.client, self.claim);
        let heartbeat_every = claim.lease / 4;
        let mut batch: Vec<String> = Vec::new();
        let mut told_to_stop = false;
        let mut gone: Option<(String, Instant)> = None;
        let mut killed = false;
        let mut held: Vec<String> = Vec::new();
        loop {
            let mut ended = false;
            match lines.recv_timeout(LOOK) {
                Ok(line) => {
                    batch.push(line);
                    while batch.len() < MOST_EVENTS
                        && let Ok(line) = lines.try_recv()
                    {
                        batch.push(line);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => ended = true,
            }

            if gone.is_some() {
                // What it says now is not sent: the control plane does not hold the run.
                batch.clear();
            } else if told_to_stop {
                held.append(&mut batch);
                if self.last_ok.elapsed() >= heartbeat_every
                    && let Delivered::Gone(why) = self.deliver(|| client.heartbeat(&claim.id))
                {
                    gone = Some((why, Instant::now()));
                    terminate(child);
                }
            } else if !batch.is_empty() {
                let numbered: Vec<String> =
                    batch.drain(..).map(|line| self.numbered(&line)).collect();
                for line in &numbered {
                    show(line, self.started);
                }
                if let Delivered::Gone(why) = self.deliver(|| client.events(&claim.id, &numbered)) {
                    gone = Some((why, Instant::now()));
                    terminate(child);
                }
            } else if self.last_ok.elapsed() >= heartbeat_every
                && let Delivered::Gone(why) = self.deliver(|| client.heartbeat(&claim.id))
            {
                gone = Some((why, Instant::now()));
                terminate(child);
            }

            if stop() && !told_to_stop && gone.is_none() {
                told_to_stop = true;
                println!("stopping: telling the executor to stop what it is running…");
                terminate(child);
            }
            if let Some((_, since)) = &gone
                && !killed
                && since.elapsed() >= GRACE
            {
                killed = true;
                let _ = child.kill();
            }
            if ended {
                return Followed {
                    gone: gone.map(|(why, _)| why),
                    stopped_by_operator: told_to_stop,
                    held,
                };
            }
        }
    }

    /// Tell the control plane the workload is given back, and say so.
    fn give_back(&mut self) {
        let (client, claim) = (self.client, self.claim);
        match self.deliver(|| client.release(&claim.id)) {
            Delivered::Yes => println!("workload {}: stopped, given back", claim.id),
            Delivered::Gone(why) => println!(
                "workload {}: not given back, because {why}; its lease lapses",
                claim.id
            ),
        }
    }

    /// Make a request that belongs to the run, again and again while the control plane cannot
    /// be reached, for as long as a lease lasts.
    fn deliver(&mut self, request: impl Fn() -> Result<(), Failure>) -> Delivered {
        let mut failures: u32 = 0;
        loop {
            match request() {
                Ok(()) => {
                    self.last_ok = Instant::now();
                    return Delivered::Yes;
                }
                Err(Failure::Trouble(why)) => {
                    if self.last_ok.elapsed() >= self.claim.lease {
                        return Delivered::Gone(format!(
                            "the control plane could not be reached for as long as a lease lasts ({why})"
                        ));
                    }
                    failures = failures.saturating_add(1);
                    std::thread::sleep(backoff(failures, BACKOFF_FIRST, BACKOFF_IN_A_RUN));
                }
                Err(other) => return Delivered::Gone(other.to_string()),
            }
        }
    }

    /// The line as the control plane is to keep it: the executor numbers its events from 1, and
    /// a workload that was put back goes on from where the lost run left off.
    fn numbered(&self, line: &str) -> String {
        let offset = self.claim.next_sequence - 1;
        if offset == 0 {
            return line.to_owned();
        }
        let Ok(mut event) = serde_json::from_str::<Value>(line) else {
            return line.to_owned();
        };
        let sequence = match &event["sequence"] {
            Value::String(text) => text.parse::<u64>().ok(),
            number => number.as_u64(),
        };
        let Some(sequence) = sequence else {
            return line.to_owned();
        };
        // The contract's JSON mapping writes a 64-bit number as a string.
        event["sequence"] = Value::String((sequence + offset).to_string());
        event.to_string()
    }
}

/// Tell the executor to stop what it started, once. It says how it ended and exits.
fn terminate(child: &Child) {
    if let Some(pid) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
        let _ = kill_process(pid, Signal::TERM);
    }
}

/// Show the event, as `heph run` does.
fn show(line: &str, started: Instant) {
    if let Some(shown) = serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|event| run::render(&event))
    {
        let seconds = started.elapsed().as_secs();
        println!("[{:02}:{:02}] {shown}", seconds / 60, seconds % 60);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    /// A run directory with a source file, a record and every kind of build output.
    fn run_with_build_output(name: &str) -> PathBuf {
        let run = std::env::temp_dir().join(format!("heph-worker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&run);
        std::fs::create_dir_all(run.join("work/repo/src")).expect("directories");
        std::fs::write(run.join("work/repo/src/main.rs"), "source").expect("source");
        std::fs::write(run.join("record.json"), "{}").expect("record");
        for built in [
            "repo/target",
            "repo/_build",
            "repo/deps",
            "repo/node_modules",
            ".tool-home",
        ] {
            std::fs::create_dir_all(run.join("work").join(built)).expect("build output");
            std::fs::write(run.join("work").join(built).join("object"), "built").expect("file");
        }
        run
    }

    #[test]
    fn a_run_that_ended_without_completing_loses_its_build_output_and_keeps_the_rest() {
        for outcome in ["failed", "timed_out", "blocked"] {
            let run = run_with_build_output(&format!("discard-{outcome}"));
            discard_build_output_of_an_unfinished_run(outcome, &run);
            assert!(
                run.join("work/repo/src/main.rs").is_file(),
                "{outcome}: source"
            );
            assert!(run.join("record.json").is_file(), "{outcome}: record");
            for built in [
                "repo/target",
                "repo/_build",
                "repo/deps",
                "repo/node_modules",
                ".tool-home",
            ] {
                assert!(!run.join("work").join(built).exists(), "{outcome}: {built}");
            }
            let _ = std::fs::remove_dir_all(&run);
        }
    }

    #[test]
    fn a_run_that_completed_or_said_nothing_keeps_its_build_output() {
        for outcome in ["completed", "unspecified"] {
            let run = run_with_build_output(&format!("keep-{outcome}"));
            discard_build_output_of_an_unfinished_run(outcome, &run);
            for built in [
                "repo/target",
                "repo/_build",
                "repo/deps",
                "repo/node_modules",
                ".tool-home",
            ] {
                assert!(run.join("work").join(built).is_dir(), "{outcome}: {built}");
            }
            let _ = std::fs::remove_dir_all(&run);
        }
    }

    fn file(name: &str, mode: u32, text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("heph-worker-{name}-{}", std::process::id()));
        std::fs::write(&path, text).expect("a file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
        path
    }

    /// Start `sleep` with `directory` as its working directory.
    fn sleeper(directory: &Path) -> Child {
        std::process::Command::new("sleep")
            .arg("100")
            .current_dir(directory)
            .spawn()
            .expect("sleep")
    }

    fn ended_within_five_seconds(child: &mut Child) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().expect("wait").is_some() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn options_for(runs: &Path) -> Options {
        Options {
            control_plane: String::new(),
            runs_dir: Some(runs.to_path_buf()),
            executor: None,
            workloads: 1,
            keep_runs_for: Duration::from_hours(168),
        }
    }

    #[test]
    fn a_process_left_by_a_finished_run_is_stopped_and_one_in_a_run_another_worker_holds_is_not() {
        let runs = std::env::temp_dir().join(format!("heph-sweep-{}", std::process::id()));
        let finished = runs.join("owner-repo").join("1-finished");
        let live = runs.join("owner-repo").join("2-live");
        std::fs::create_dir_all(finished.join("work")).expect("a directory");
        std::fs::create_dir_all(&live).expect("a directory");
        // The finished run's holder let go; the live run's holder is another worker, which
        // takes its own lock on the file just as a worker in another process does.
        drop(run_lock::hold(&finished).expect("held"));
        // A test beside this one may be between fork and exec, a child that holds the lock's
        // open file for that moment, so the release is waited for.
        let deadline = Instant::now() + Duration::from_secs(10);
        while run_lock::is_held(&finished) {
            assert!(Instant::now() < deadline, "the finished run stays held");
            std::thread::sleep(Duration::from_millis(10));
        }
        let other_workers_hold = run_lock::hold(&live).expect("held");
        let mut left = sleeper(&finished.join("work"));
        let left_id = left.id();
        let mut kept = sleeper(&live);

        let mut said = Vec::new();
        stop_what_finished_runs_left_logging(&options_for(&runs), &mut said);
        let said = String::from_utf8_lossy(&said).into_owned();

        let left_stopped = ended_within_five_seconds(&mut left);
        let kept_alive = kept.try_wait().expect("wait").is_none();
        drop(other_workers_hold);
        let _ = kept.kill();
        let _ = left.kill();
        let _ = std::fs::remove_dir_all(&runs);
        assert!(left_stopped, "the finished run's process was left");
        assert!(
            said.contains(&format!("stopping process {left_id}"))
                && said.contains("nobody holds the run")
                && said.contains("1-finished"),
            "the stop was not logged with its run and reason: {said}"
        );
        assert!(
            !said.contains("2-live"),
            "the live run was logged as stopped: {said}"
        );
        assert!(
            kept_alive,
            "the other worker's live run's process was stopped"
        );
    }

    #[test]
    fn a_process_left_by_a_killed_worker_is_stopped() {
        use std::io::{BufRead as _, BufReader};
        use std::process::Stdio;

        let runs = std::env::temp_dir().join(format!("heph-killed-{}", std::process::id()));
        let died = runs.join("owner-repo").join("3-died");
        std::fs::create_dir_all(&died).expect("a directory");
        // A real process, apart from this one, takes the lock: this test binary run again
        // on the helper below, which holds the directory until it is killed.
        let mut holder = std::process::Command::new(std::env::current_exe().expect("exe"))
            .args([
                "--exact",
                "worker::tests::lock_holder_for_another_test",
                "--nocapture",
            ])
            .env("HEPH_TEST_HOLD", &died)
            .stdout(Stdio::piped())
            .spawn()
            .expect("the holder");
        let lines = BufReader::new(holder.stdout.take().expect("stdout")).lines();
        let held = lines
            .map_while(Result::ok)
            .any(|line| line.contains("holding-the-run-directory"));
        let mut left = sleeper(&died);
        let left_id = left.id();

        let spared = held && {
            stop_what_finished_runs_left(&options_for(&runs));
            std::thread::sleep(Duration::from_millis(500));
            left.try_wait().expect("wait").is_none()
        };
        holder.kill().expect("SIGKILL the holder");
        let _ = holder.wait();
        let mut said = Vec::new();
        stop_what_finished_runs_left_logging(&options_for(&runs), &mut said);
        let said = String::from_utf8_lossy(&said).into_owned();

        let left_stopped = ended_within_five_seconds(&mut left);
        let _ = left.kill();
        let _ = std::fs::remove_dir_all(&runs);
        assert!(held, "the holder never took the lock");
        assert!(spared, "the sweep stopped a process while its worker lived");
        assert!(left_stopped, "the killed worker's process was left");
        assert!(
            said.contains(&format!("stopping process {left_id}"))
                && said.contains("nobody holds the run")
                && said.contains("3-died"),
            "the stop was not logged with its run and reason: {said}"
        );
    }

    /// Not a test of its own: the process the one above starts and kills. Run alone it does
    /// nothing, as the variable is not set.
    #[test]
    fn lock_holder_for_another_test() {
        let Some(directory) = std::env::var_os("HEPH_TEST_HOLD") else {
            return;
        };
        let _held = run_lock::hold(Path::new(&directory)).expect("held");
        println!("holding-the-run-directory");
        std::thread::sleep(Duration::from_secs(120));
    }

    #[test]
    fn the_token_is_the_variable_else_the_named_file_else_the_default_file() {
        let named = file("named", 0o600, "hdt_from-file\n");
        let default = file("default", 0o600, "hdt_default\n");
        let shown = |token: Result<Token, String>| format!("{token:?}");
        assert!(
            token_from(Some("hdt_v".into()), None, Some(default.clone()))
                .expect("a token")
                .is_in("hdt_v")
        );
        assert!(
            token_from(
                None,
                Some(named.display().to_string()),
                Some(default.clone())
            )
            .expect("a token")
            .is_in("hdt_from-file")
        );
        assert!(
            token_from(None, None, Some(default.clone()))
                .expect("a token")
                .is_in("hdt_default")
        );
        assert!(!shown(token_from(None, None, Some(default))).contains("hdt_default"));
    }

    #[test]
    fn two_ways_of_giving_the_token_are_an_error_and_none_says_where_to_put_it() {
        let error = token_from(Some("a".into()), Some("/x".into()), None).expect_err("both");
        assert!(error.contains("give one"), "{error}");
        let error =
            token_from(None, None, Some(PathBuf::from("/nowhere/worker-token"))).expect_err("none");
        assert!(error.contains("/nowhere/worker-token"), "{error}");
        assert!(error.contains("mode 600"), "{error}");
    }

    #[test]
    fn a_token_file_others_can_read_or_that_is_empty_is_refused_without_showing_it() {
        let open = file("open", 0o644, "hdt_the-secret\n");
        let error = token_from(None, Some(open.display().to_string()), None).expect_err("open");
        assert!(error.contains("chmod 600"), "{error}");
        assert!(!error.contains("hdt_the-secret"), "{error}");
        let empty = file("empty", 0o600, "\n");
        let error = token_from(None, Some(empty.display().to_string()), None).expect_err("empty");
        assert!(error.contains("is empty"), "{error}");
    }

    #[test]
    fn a_wait_doubles_to_a_cap_and_is_never_less_than_half_of_it() {
        let first = Duration::from_secs(1);
        let longest = Duration::from_secs(60);
        for _ in 0..200 {
            for (failures, whole) in [(1, 1), (2, 2), (3, 4), (6, 32), (7, 60), (40, 60)] {
                let wait = backoff(failures, first, longest);
                let whole = Duration::from_secs(whole);
                assert!(wait >= whole / 2 && wait <= whole, "{failures}: {wait:?}");
            }
        }
        let varied: std::collections::BTreeSet<u128> = (0..50)
            .map(|_| backoff(5, first, longest).as_micros())
            .collect();
        assert!(varied.len() > 1, "the wait is the same every time");
    }

    #[test]
    fn workloads_default_to_one_and_the_flag_wins_over_the_variable() {
        assert_eq!(workloads_from(None, None), Ok(1));
        assert_eq!(workloads_from(None, Some("3")), Ok(3));
        assert_eq!(workloads_from(Some("2"), Some("3")), Ok(2));
        assert_eq!(workloads_from(Some("64"), None), Ok(64));
    }

    #[test]
    fn zero_workloads_is_refused_and_says_so() {
        let error = workloads_from(Some("0"), None).expect_err("zero");
        assert!(
            error.contains("at least one") && error.contains("--workloads"),
            "{error}"
        );
    }

    #[test]
    fn workloads_above_the_ceiling_are_refused_and_say_so() {
        let error = workloads_from(Some("65"), None).expect_err("too many");
        assert!(
            error.contains("above the most") && error.contains("64"),
            "{error}"
        );
        let error = workloads_from(None, Some("640")).expect_err("too many");
        assert!(error.contains(WORKLOADS_VARIABLE), "{error}");
        assert!(workloads_from(Some("many"), None).is_err());
        assert!(workloads_from(Some("-1"), None).is_err());
    }

    #[test]
    fn only_plain_names_make_a_directory() {
        assert_eq!(safe("0d7a1c3e-6f2b"), Some("0d7a1c3e-6f2b"));
        assert_eq!(safe(".."), None);
        assert_eq!(safe("a/b"), None);
        assert_eq!(safe(""), None);
    }
}
