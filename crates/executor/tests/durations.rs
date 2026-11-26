//! What the run record says about where the time went: in the agent, in the gates and in
//! reading the change, so that a run that ends on its timeout can be told from one that was
//! slow in a place it could have avoided.

mod common;

use common::{agent_script, manifest, rig, run};

/// A gate that takes about a second.
const SLOW_GATE: &str =
    "\n[gates.slow]\nrun = [\"sh\", \"-c\", \"sleep 1\"]\ntimeout_seconds = 30\n";

#[test]
fn the_record_carries_the_time_spent_in_the_agent_and_in_the_gates_and_none_in_review() {
    let rig = rig("durations", &(manifest(&[]) + SLOW_GATE));

    let (ran, _, _) = run(rig, &agent_script("sleep 1;"), false, 0, &[]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let record = ran.record();
    assert!(
        (1_000..30_000).contains(&record.agent_ms),
        "the agent slept a second: {}",
        record.agent_ms
    );
    assert!(
        (1_000..30_000).contains(&record.gates_ms),
        "the gate slept a second: {}",
        record.gates_ms
    );
    assert_eq!(record.review_ms, 0, "no reader ran");
}
