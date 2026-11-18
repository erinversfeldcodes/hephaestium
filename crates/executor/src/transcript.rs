//! A recorded transcript, played in place of an agent.
//!
//! This is for development and for tests. It is not a runtime kind in the
//! contract, so nothing in a workload can select it.

use contracts::v1alpha1::Event;
use contracts::v1alpha1::event::Kind;

use crate::stream::kind_name;

/// The events one line of an adapter's output means, for the kinds an adapter
/// may produce. The executor alone says when a run starts, what the workspace
/// is, what the gates said and how it ended, so a line that holds one of those,
/// or holds no event, is an error. Everything but the kind is ignored: the
/// executor gives each event its own workload, sequence and time.
///
/// # Errors
///
/// Returns why the line is not an event an adapter may produce.
pub fn kind_of(line: &str) -> Result<Kind, String> {
    let event: Event = serde_json::from_str(line).map_err(|error| error.to_string())?;
    match event.kind {
        Some(
            kind @ (Kind::AgentMessage(_)
            | Kind::ToolCall(_)
            | Kind::ToolResult(_)
            | Kind::SkillLoaded(_)
            | Kind::Cost(_)),
        ) => Ok(kind),
        Some(other) => Err(format!(
            "holds a `{}` event, which an adapter does not produce",
            kind_name(&other)
        )),
        None => Err("holds no event".to_owned()),
    }
}

/// Check a transcript before anything is run: a line of JSON for each event,
/// each of a kind an adapter may produce.
///
/// # Errors
///
/// Returns the first line that is not.
pub fn check(text: &str) -> Result<(), String> {
    for (index, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        kind_of(line).map_err(|error| format!("transcript line {}: {error}", index + 1))?;
    }
    Ok(())
}
