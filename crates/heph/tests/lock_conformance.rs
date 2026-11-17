//! Grade the lock check against Athenaeum's cases, as the manifest check is.
//!
//! An accepted case must be accepted. A rejected case must be rejected with
//! exactly one violation, at the path and by the keyword the case states.

use athenaeum::Expectation;
use athenaeum::lock::CASES;
use heph::lock::{Invalid, check};

#[test]
fn every_case_is_judged_as_it_states() {
    let mut disagreements = Vec::new();

    for case in CASES {
        let verdict = check(case.toml);
        match (case.expectation, &verdict) {
            (Expectation::Accepted, Ok(())) => {}
            (Expectation::Accepted, Err(reason)) => {
                disagreements.push(format!("{}: must be accepted, but: {reason:?}", case.name));
            }
            (Expectation::Rejected { path, keyword }, Err(Invalid::Violations(violations)))
                if violations.len() == 1
                    && violations[0].path == path
                    && violations[0].keyword == keyword => {}
            (Expectation::Rejected { path, keyword }, other) => {
                disagreements.push(format!(
                    "{}: must be rejected once at {path} by {keyword}, but: {other:?}",
                    case.name
                ));
            }
        }
    }

    assert!(
        disagreements.is_empty(),
        "{} of {} cases were judged wrongly:\n{}",
        disagreements.len(),
        CASES.len(),
        disagreements.join("\n")
    );
}

#[test]
fn every_lock_the_schema_accepts_can_be_read() {
    for case in CASES
        .iter()
        .filter(|case| matches!(case.expectation, Expectation::Accepted))
    {
        let parsed = heph::lock::parse(case.toml);
        assert!(
            parsed.is_ok(),
            "{} is accepted by the schema but cannot be read: {parsed:?}",
            case.name
        );
    }
}

#[test]
fn text_that_is_not_toml_is_told_apart_from_a_lock_that_breaks_a_rule() {
    let verdict = check("rev = [unterminated");

    assert!(
        matches!(verdict, Err(Invalid::NotToml(_))),
        "expected NotToml, but: {verdict:?}"
    );
}
