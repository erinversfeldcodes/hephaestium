//! A recorded transcript, played in place of an agent.
//!
//! This is for development and for tests. It is not a runtime kind in the
//! contract, so nothing in a workload can select it.

use contracts::v1alpha1::Event;
use contracts::v1alpha1::event::Kind;

use crate::stream::kind_name;

/// Read a transcript: a line of JSON for each event. Only the kinds an adapter
/// can produce are allowed, because the executor alone says when a run starts,
/// what the workspace is, what the gates said and how it ended. Everything but
/// the kind is ignored: the executor gives each event its own workload,
/// sequence and time.
///
/// # Errors
///
/// Returns the first line that is not an event, or that holds a kind an adapter
/// does not produce.
pub fn read(text: &str) -> Result<Vec<Kind>, String> {
    let mut kinds = Vec::new();
    for (index, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let number = index + 1;
        let event: Event = serde_json::from_str(line)
            .map_err(|error| format!("transcript line {number}: {error}"))?;
        let kind = event
            .kind
            .ok_or_else(|| format!("transcript line {number}: holds no event"))?;
        match kind {
            Kind::AgentMessage(_)
            | Kind::ToolCall(_)
            | Kind::ToolResult(_)
            | Kind::SkillLoaded(_)
            | Kind::Cost(_) => kinds.push(kind),
            other => {
                return Err(format!(
                    "transcript line {number}: holds a `{}` event, which an adapter does not produce",
                    kind_name(&other)
                ));
            }
        }
    }
    Ok(kinds)
}
