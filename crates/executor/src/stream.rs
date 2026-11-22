//! The event stream: what the executor says it did, in order, and the tally of
//! it that the run record is made from.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::redact::Redactor;
use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::finished::Outcome;
use contracts::v1alpha1::{
    CostTotal, Event, Finished, GateRun, PullRequestOpened, RunRecord, SkillLoaded, TokenBreakdown,
    Workload,
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
        Kind::StepStarted(_) => "step_started",
        Kind::StepFinished(_) => "step_finished",
        Kind::DeliveryBegan(_) => "delivery_began",
        Kind::Finished(_) => "finished",
    }
}

/// What has been spent in one unit from one source: the amount, and for tokens the parts of it
/// that were given. When only some of the events gave parts, the parts are theirs alone and do
/// not add up to the amount, and they are never made up for the ones that did not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spent {
    pub amount: f64,
    pub breakdown: Option<TokenBreakdown>,
}

impl Spent {
    fn add(&mut self, cost: &contracts::v1alpha1::Cost) {
        self.amount += cost.amount;
        if let Some(parts) = &cost.breakdown {
            let total = self.breakdown.get_or_insert_with(TokenBreakdown::default);
            total.input += parts.input;
            total.output += parts.output;
            total.cache_creation += parts.cache_creation;
            total.cache_read += parts.cache_read;
        }
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
    costs: BTreeMap<(i32, i32), Spent>,
    skills: Vec<SkillLoaded>,
    gates: Vec<GateRun>,
    pull_request: Option<PullRequestOpened>,
    // The last thing the agent said in words: its account of what it did or found.
    last_message: Option<String>,
    // What the run asks a person, when it ended by asking.
    question: String,
    redactor: Redactor,
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
            last_message: None,
            question: String::new(),
            redactor: Redactor::default(),
        }
    }

    /// The last thing the agent said in words, if it said anything besides whitespace.
    #[must_use]
    pub fn account(&self) -> Option<&str> {
        self.last_message
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }

    /// Say what the run asks a person: the question is written in the last event and the
    /// run record when the run finishes.
    pub fn ask(&mut self, question: &str) {
        question.clone_into(&mut self.question);
    }

    /// Take the value of every secret `redactor` holds out of each event before it is written,
    /// so that nothing the agent or a gate prints can carry one into the stream.
    #[must_use]
    pub fn redacting(mut self, redactor: Redactor) -> Self {
        self.redactor = redactor;
        self
    }

    /// What has been spent so far, for each unit and source, so that what a step spent is
    /// the difference between two looks.
    #[must_use]
    pub fn costs(&self) -> &BTreeMap<(i32, i32), Spent> {
        &self.costs
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
                self.costs
                    .entry((cost.unit, cost.source))
                    .or_default()
                    .add(cost);
            }
            Kind::SkillLoaded(skill) => self.skills.push(skill.clone()),
            Kind::PullRequestOpened(pr) => self.pull_request = Some(pr.clone()),
            Kind::AgentMessage(said) => self.last_message = Some(said.text.clone()),
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

        let mut line = serde_json::to_string(&event).map_err(std::io::Error::other)?;
        if !self.redactor.is_empty() {
            let mut value = serde_json::to_value(&event).map_err(std::io::Error::other)?;
            // An event that holds no secret is written as it was made.
            if self.redactor.json(&mut value) {
                line = serde_json::to_string(&value).map_err(std::io::Error::other)?;
            }
        }
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
            question: self.question.clone(),
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
            question: self.question,
            started_at: self.started_at,
            finished_at: Some(finished_at),
            gates: self.gates,
            costs: self
                .costs
                .into_iter()
                .map(|((unit, source), spent)| CostTotal {
                    unit,
                    amount: spent.amount,
                    source,
                    breakdown: spent.breakdown,
                })
                .collect(),
            skills: self.skills,
            pull_request: self.pull_request,
            event_count: self.next - 1,
            executor_version: executor_version.to_owned(),
            configuration_sha256: String::new(),
            review: None,
            completeness: None,
            gap: None,
            // A run that starts one agent has no steps to tell apart: its costs are that
            // agent's. The executor lists them for a run that reads its own change.
            steps: Vec::new(),
        })
    }
}
