//! The event stream: what the executor says it did, in order, and the tally of
//! it that the run record is made from.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::{
    CostTotal, Event, Finished, GateRun, PullRequestOpened, RunRecord, SkillLoaded, Workload,
};
use pbjson_types::Timestamp;

/// The name a kind has in the contract.
#[must_use]
pub fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Started(_) => "started",
        Kind::WorkspaceReady(_) => "workspace_ready",
        Kind::AgentMessage(_) => "agent_message",
        Kind::ToolCall(_) => "tool_call",
        Kind::ToolResult(_) => "tool_result",
        Kind::SkillLoaded(_) => "skill_loaded",
        Kind::GateStarted(_) => "gate_started",
        Kind::GateFinished(_) => "gate_finished",
        Kind::Cost(_) => "cost",
        Kind::Heartbeat(_) => "heartbeat",
        Kind::PullRequestOpened(_) => "pull_request_opened",
        Kind::Finished(_) => "finished",
    }
}

/// Writes a workload's events, one JSON object to a line, and keeps what the
/// run record needs.
///
/// The sequence is gapless from 1 and the times never go backwards, because the
/// stream is the only place either is decided.
pub struct Stream<'a, W: Write> {
    out: &'a mut W,
    workload_id: String,
    next: u64,
    last: (i64, i32),
    started_at: Option<Timestamp>,
    // Totalled for each unit and source and never across them: dollars, tokens
    // and credits are different things, and so are what a runtime reported and
    // what was estimated.
    costs: BTreeMap<(i32, i32), f64>,
    skills: Vec<SkillLoaded>,
    gates: Vec<GateRun>,
    pull_request: Option<PullRequestOpened>,
}

impl<'a, W: Write> Stream<'a, W> {
    pub fn new(out: &'a mut W, workload_id: &str) -> Self {
        Self {
            out,
            workload_id: workload_id.to_owned(),
            next: 1,
            last: (0, 0),
            started_at: None,
            costs: BTreeMap::new(),
            skills: Vec::new(),
            gates: Vec::new(),
            pull_request: None,
        }
    }

    /// Write the next event.
    ///
    /// # Errors
    ///
    /// Returns the error if the event could not be written.
    pub fn emit(&mut self, kind: Kind) -> std::io::Result<()> {
        let time = self.now();
        match &kind {
            Kind::Cost(cost) => {
                *self.costs.entry((cost.unit, cost.source)).or_insert(0.0) += cost.amount;
            }
            Kind::SkillLoaded(skill) => self.skills.push(skill.clone()),
            Kind::PullRequestOpened(pr) => self.pull_request = Some(pr.clone()),
            Kind::GateFinished(gate) => self.gates.push(GateRun {
                name: gate.name.clone(),
                result: gate.result.clone(),
            }),
            _ => {}
        }
        let event = Event {
            workload_id: self.workload_id.clone(),
            sequence: self.next,
            time: Some(time),
            kind: Some(kind),
        };
        self.next += 1;
        self.started_at.get_or_insert(time);

        let line = serde_json::to_string(&event).map_err(std::io::Error::other)?;
        writeln!(self.out, "{line}")?;
        self.out.flush()
    }

    /// The time of the next event: now, and never before the last.
    fn now(&mut self) -> Timestamp {
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let now = (
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
            i32::try_from(since.subsec_nanos()).unwrap_or(0),
        );
        self.last = self.last.max(now);
        Timestamp {
            seconds: self.last.0,
            nanos: self.last.1,
        }
    }

    /// Write the last event and make the run record.
    ///
    /// # Errors
    ///
    /// Returns the error if the event could not be written.
    pub fn finish(
        mut self,
        workload: &Workload,
        outcome: Outcome,
        reason: &str,
        executor_version: &str,
    ) -> std::io::Result<RunRecord> {
        self.emit(Kind::Finished(Finished {
            outcome: outcome as i32,
            reason: reason.to_owned(),
            question: String::new(),
        }))?;
        let finished_at = Timestamp {
            seconds: self.last.0,
            nanos: self.last.1,
        };
        Ok(RunRecord {
            workload_id: self.workload_id,
            workload: Some(workload.clone()),
            outcome: outcome as i32,
            reason: reason.to_owned(),
            question: String::new(),
            started_at: self.started_at,
            finished_at: Some(finished_at),
            gates: self.gates,
            costs: self
                .costs
                .into_iter()
                .map(|((unit, source), amount)| CostTotal {
                    unit,
                    amount,
                    source,
                })
                .collect(),
            skills: self.skills,
            pull_request: self.pull_request,
            event_count: self.next - 1,
            executor_version: executor_version.to_owned(),
            configuration_sha256: String::new(),
            review: None,
            completeness: None,
        })
    }
}
