//! Grade the manifest check against Athenaeum's cases.
//!
//! The cases come from the Athenaeum tag this crate pins, so they are the
//! contract the binary was built for. An accepted case must be accepted. A
//! rejected case must be rejected with exactly one violation, at the path and
//! by the keyword the case states: rejecting it for some other reason would
//! mean the rule it was written for is not the one being enforced.

use athenaeum::manifest::{CASES, Expectation};
use heph::manifest::{Invalid, check};

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
fn the_pinned_tag_carries_cases_of_both_kinds() {
    let rejected = CASES
        .iter()
        .filter(|case| matches!(case.expectation, Expectation::Rejected { .. }))
        .count();

    assert!(
        rejected > 0,
        "no rejected cases, so nothing proves a rule is enforced"
    );
    assert!(
        rejected < CASES.len(),
        "no accepted cases, so rejecting everything would pass"
    );
}

#[test]
fn text_that_is_not_toml_is_told_apart_from_a_manifest_that_breaks_a_rule() {
    let verdict = check("schema_version = [unterminated");

    assert!(
        matches!(verdict, Err(Invalid::NotToml(_))),
        "expected NotToml, but: {verdict:?}"
    );
}

#[test]
fn every_manifest_the_schema_accepts_can_be_read_for_what_the_commands_need() {
    for case in CASES
        .iter()
        .filter(|case| matches!(case.expectation, Expectation::Accepted))
    {
        let parsed = heph::manifest::parse(case.toml);
        assert!(
            parsed.is_ok(),
            "{} is accepted by the schema but cannot be read: {parsed:?}",
            case.name
        );
    }
}
