//! What a run ran under, taken from the build and from what the run did.

use contracts::v1alpha1::{Provenance, RunRecord};

/// The provenance of a run: the release and build it ran under, the skills it loaded and the
/// lenses it read under, each by name.
#[must_use]
pub fn of(record: &RunRecord) -> Provenance {
    let mut lenses: Vec<String> = Vec::new();
    let reviewed = record
        .workload
        .as_ref()
        .and_then(|w| w.review.as_ref())
        .map(|r| r.lens.clone());
    let read = record.steps.iter().map(|s| s.lens.clone());
    for lens in reviewed.into_iter().chain(read) {
        if !lens.is_empty() && !lenses.contains(&lens) {
            lenses.push(lens);
        }
    }
    Provenance {
        athenaeum_tag: heph::build::ATHENAEUM_TAG.to_owned(),
        athenaeum_commit: heph::build::ATHENAEUM_COMMIT.to_owned(),
        executor_commit: heph::build::COMMIT.to_owned(),
        skills: record.skills.iter().map(|s| s.name.clone()).collect(),
        lenses,
    }
}
