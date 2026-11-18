//! Running the agent: start it through its adapter, relay what it says as it
//! says it, and stop it, and everything it started, at the workload's limits.

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::Path;
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

/// Run the agent in the workspace.
///
/// It holds only the environment variables its plan names, and only those the
/// executor has. The workload's timeout bounds it. The budget is what it reports
/// spending in the budget's own unit, never converted from another: once it is
/// passed the agent is stopped. A heartbeat is written every `heartbeat` while
/// it runs.
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
    stream_out: &mut Stream<'_, W>,
) -> Result<End, Refused> {
    if let Err(error) = adapter.prepare(workload, workspace) {
        return Ok(End::Failed(format!(
            "the agent could not be prepared: {error}"
        )));
    }
    let plan = adapter.plan(workload, workspace);
    let environment: Vec<(String, String)> = plan
        .env_names
        .iter()
        .filter_map(|name| {
            secrets
                .get(name)
                .map(str::to_owned)
                .or_else(|| std::env::var(name).ok())
                .map(|value| (name.clone(), value))
        })
        .collect();

    let limits = workload.limits.as_ref().expect("checked when read");
    let timeout_seconds = limits.timeout.as_ref().expect("checked when read");
    let timeout = Duration::new(
        u64::try_from(timeout_seconds.seconds).unwrap_or(0),
        u32::try_from(timeout_seconds.nanos).unwrap_or(0),
    );
    let budget = limits.budget.as_ref().expect("checked when read");

    let out = RefCell::new(stream_out);
    let line_number = Cell::new(0_usize);
    let spent = Cell::new(0.0_f64);
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

    if let Some(error) = write_failed.into_inner() {
        return Err(error.into());
    }
    Ok(match ended {
        Streamed::Exited(0) => End::Done,
        Streamed::Exited(code) => End::Failed(format!("the agent exited with code {code}")),
        Streamed::TimedOut => End::TimedOut(format!(
            "the agent ran past the workload's timeout of {timeout:?} and was stopped"
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
