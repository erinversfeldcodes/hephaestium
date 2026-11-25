//! Running the agent: start it through its adapter, relay what it says as it
//! says it, and stop it, and everything it started, at the workload's limits.

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::{Heartbeat, Workload};
use heph::process::{Go, Streamed, stream};

use crate::adapter::Adapter;
use crate::run::{Refused, say};
use crate::secrets::Secrets;
use crate::stream::Stream;

/// How the agent's part of the run ended.
pub enum End {
    /// It exited cleanly, within its limits.
    Done,
    /// It did not finish its work, and why.
    Failed(String),
    /// It ran past the workload's timeout.
    TimedOut(String),
    /// The executor was told to stop, and the agent was stopped with everything it
    /// started.
    Cancelled,
}

/// Where the agent asks how long is left: a script beside the workspace, so that it is not part
/// of the repository. It is written for every agent, so that no adapter has to tell time.
#[must_use]
pub fn clock_path(workspace: &Path) -> PathBuf {
    let name = workspace
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    workspace.with_file_name(format!("{name}.time-left"))
}

/// The script that says how long is left, and, once half and then three quarters of the time
/// have gone, says so. It works the answer out when it is run, from the instants the run
/// starts and must end, so it is right whenever it is asked and needs no process of its own.
fn clock_script(started: u64, ends: u64) -> String {
    format!(
        r#"#!/bin/sh
started={started}
ends={ends}
now=$(date +%s)
total=$((ends - started))
elapsed=$((now - started))
left=$((ends - now))
if [ "$left" -le 0 ]; then
  echo "Your time is up."
  exit 0
fi
echo "$(( (left + 59) / 60 )) minute(s) left of $(( (total + 59) / 60 )) minute(s), and the run ends at $(date -u -r "$ends" '+%H:%M UTC' 2>/dev/null || date -u -d "@$ends" '+%H:%M UTC')."
if [ $((elapsed * 4)) -ge $((total * 3)) ]; then
  echo "Three quarters of your time have gone. Stop verifying now: do not start another suite or wait on anything. Deliver what you have, write the commit message, description and resolution, and say in the description what you did not run."
elif [ $((elapsed * 2)) -ge "$total" ]; then
  echo "Half of your time has gone."
fi
"#
    )
}

/// Write the clock the agent asks, for a run that has `timeout` from now.
fn write_clock(workspace: &Path, timeout: Duration) -> std::io::Result<()> {
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    std::fs::write(
        clock_path(workspace),
        clock_script(started, started + timeout.as_secs()),
    )
}

/// The environment the agent has: the secrets its adapter names, and for an evaluation
/// the read-only token.
fn environment_for(
    names: &[String],
    workload: &Workload,
    secrets: &Secrets,
) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = names
        .iter()
        .filter_map(|name| {
            secrets
                .get(name)
                .map(str::to_owned)
                .or_else(|| std::env::var(name).ok())
                .map(|value| (name.clone(), value))
        })
        .collect();
    // An evaluation reads runs, caches, pull requests and issues, and a step of the questioning
    // reads a ticket's thread, so those alone are given the read-only token, under the name `gh` looks for. It is the executor and not the
    // adapter that decides, so no adapter's plan can hand it to another kind of workload,
    // and the token that writes is never part of it.
    if (workload.completeness.is_some() || workload.gap.is_some())
        && let Some(token) = secrets.get(crate::secrets::READ_TOKEN)
    {
        environment.push(("GH_TOKEN".to_owned(), token.to_owned()));
    }
    environment
}

/// Run the agent in the workspace.
///
/// It holds only the environment variables its plan names, and only those the
/// executor has. The workload's timeout bounds it. The budget is what it reports
/// spending in the budget's own unit, never converted from another: once it is
/// passed the agent is stopped. `spent` is what the workload has spent so far, in
/// that unit, and is added to as this agent reports: a run that asks an agent twice
/// spends from one budget, not from two. A heartbeat is written every `heartbeat`
/// while it runs.
///
/// # Errors
///
/// Returns the error if an event could not be written.
///
/// # Panics
///
/// If the workload has no timeout or budget. Reading a workload refuses one
/// without them, so reaching here without them is a defect in the caller.
pub fn run<W: Write>(
    adapter: &dyn Adapter,
    workload: &Workload,
    workspace: &Path,
    heartbeat: Duration,
    secrets: &Secrets,
    spent: &Cell<f64>,
    stream_out: &mut Stream<'_, W>,
) -> Result<End, Refused> {
    let limits = workload.limits.as_ref().expect("checked when read");
    let timeout_seconds = limits.timeout.as_ref().expect("checked when read");
    let timeout = Duration::new(
        u64::try_from(timeout_seconds.seconds).unwrap_or(0),
        u32::try_from(timeout_seconds.nanos).unwrap_or(0),
    );
    if let Err(error) = adapter
        .prepare(workload, workspace)
        .and_then(|()| write_clock(workspace, timeout))
    {
        return Ok(End::Failed(format!(
            "the agent could not be prepared: {error}"
        )));
    }
    let plan = adapter.plan(workload, workspace);
    let environment = environment_for(&plan.env_names, workload, secrets);

    let budget = limits.budget.as_ref().expect("checked when read");

    let out = RefCell::new(stream_out);
    let line_number = Cell::new(0_usize);
    let stopped_because: RefCell<Option<String>> = RefCell::new(None);
    let write_failed: RefCell<Option<std::io::Error>> = RefCell::new(None);

    let emit = |kind: Kind| -> Go {
        match out.borrow_mut().emit(kind) {
            Ok(()) => Go::On,
            Err(error) => {
                *write_failed.borrow_mut() = Some(error);
                Go::Stop
            }
        }
    };

    let ended = stream(
        &plan.command,
        Path::new(&plan.working_directory),
        &environment,
        timeout,
        heartbeat,
        |line| {
            line_number.set(line_number.get() + 1);
            let kinds = match adapter.translate(line) {
                Ok(kinds) => kinds,
                Err(error) => {
                    *stopped_because.borrow_mut() = Some(format!(
                        "the agent's output line {} {error}",
                        line_number.get()
                    ));
                    return Go::Stop;
                }
            };
            for kind in kinds {
                if let Kind::Cost(cost) = &kind
                    && cost.unit == budget.unit
                {
                    spent.set(spent.get() + cost.amount);
                }
                if emit(kind) == Go::Stop {
                    return Go::Stop;
                }
            }
            if spent.get() > budget.amount {
                *stopped_because.borrow_mut() = Some(format!(
                    "the agent spent {} against a budget of {}, and was stopped",
                    spent.get(),
                    budget.amount
                ));
                return Go::Stop;
            }
            Go::On
        },
        || emit(Kind::Heartbeat(Heartbeat {})),
        say,
    )
    .map_err(|error| Refused(format!("waiting for the agent failed: {error}")))?;

    // Whatever the agent left running outside its process group goes too, however it ended,
    // before anything reads the workspace or the next step starts.
    for process in heph::process::stop_processes_under(workspace) {
        say(&format!(
            "stopped process {} the agent left running in {}",
            process.pid,
            process.directory.display()
        ));
    }

    if let Some(error) = write_failed.into_inner() {
        return Err(error.into());
    }
    Ok(match ended {
        Streamed::Exited(0) => End::Done,
        Streamed::Exited(code) => End::Failed(format!("the agent exited with code {code}")),
        Streamed::TimedOut => End::TimedOut(format!(
            "the agent ran past the workload's timeout of {timeout:?} and was stopped; the load average on this machine, over one, five and fifteen minutes, was {}",
            heph::process::load_average().unwrap_or_else(|| "not readable".to_owned())
        )),
        Streamed::Stopped => End::Failed(
            stopped_because
                .into_inner()
                .unwrap_or_else(|| "the agent was stopped".to_owned()),
        ),
        Streamed::Cancelled => End::Cancelled,
        Streamed::NotRun(_) => End::Failed(format!(
            "the agent could not be started: {}",
            plan.command.first().map_or("", String::as_str)
        )),
    })
}
