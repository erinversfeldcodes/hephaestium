//! What a workload that reads its own change before it pushes must be to be read at all.
//!
//! The lenses travel with the workload, because the executor holds no GitHub token until
//! the push and which lenses a change needs depends on what the agent changed. The strict
//! reader every run starts with refuses what could not be used safely.

mod common;

use common::workload;
use executor::workload::read;
use serde_json::{Value, json};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn lens(pack: &str) -> Value {
    json!({"pack": pack, "document": format!("---\nname: {pack}\n---\nLook at {pack}.")})
}

fn reading(edit: impl FnOnce(&mut Value)) -> Value {
    let mut value = workload(COMMIT);
    value["lenses"] = json!([lens("rust"), lens("elixir")]);
    value["prepushRounds"] = json!(2);
    edit(&mut value);
    value
}

fn refused(value: &Value) -> String {
    read(&value.to_string()).expect_err("it should be refused")
}

#[test]
fn lenses_and_rounds_are_read() {
    let workload = read(&reading(|_| {}).to_string()).expect("a workload with lenses");

    assert_eq!(workload.prepush_rounds, 2);
    let packs: Vec<_> = workload.lenses.iter().map(|l| l.pack.as_str()).collect();
    assert_eq!(packs, ["rust", "elixir"]);
    assert!(workload.lenses[0].document.contains("Look at rust"));
}

#[test]
fn a_workload_with_none_reads_nothing_before_it_pushes() {
    let workload = read(&workload(COMMIT).to_string()).expect("a workload");

    assert_eq!(workload.lenses, []);
    assert_eq!(workload.prepush_rounds, 0);
}

#[test]
fn rounds_with_no_lens_are_refused_and_not_silently_skipped() {
    let message = refused(&reading(|v| v["lenses"] = json!([])));
    assert!(message.contains("no lens"), "{message}");
}

#[test]
fn lenses_with_no_rounds_are_refused() {
    let message = refused(&reading(|v| v["prepushRounds"] = json!(0)));
    assert!(message.contains("rounds"), "{message}");
}

#[test]
fn a_negative_number_of_rounds_is_refused() {
    let message = refused(&reading(|v| v["prepushRounds"] = json!(-1)));
    assert!(message.contains("rounds"), "{message}");
}

#[test]
fn a_lens_is_named_and_never_a_path() {
    for bad in [
        "../rust",
        "a/b",
        "",
        "Rust",
        "-rust",
        "rust\n",
        &"a".repeat(42),
    ] {
        let message = refused(&reading(|v| v["lenses"] = json!([lens(bad)])));
        assert!(message.contains("lens"), "{bad:?}: {message}");
    }
}

#[test]
fn a_lens_with_no_text_is_refused() {
    let message = refused(&reading(|v| {
        v["lenses"] = json!([{"pack": "rust", "document": "  \n"}]);
    }));
    assert!(message.contains("no text"), "{message}");
}

#[test]
fn a_pack_given_twice_is_refused() {
    let message = refused(&reading(|v| {
        v["lenses"] = json!([lens("rust"), lens("rust")]);
    }));
    assert!(message.contains("twice"), "{message}");
}

#[test]
fn a_review_never_reads_its_own_change_under_lenses() {
    let message = refused(&reading(|v| {
        v["review"] = json!({
            "pullRequestNumber": "52",
            "pullRequestUrl": "https://github.com/erinversfeldcodes/gazebolang/pull/52",
            "lens": "rust",
            "lensDocument": "---\nname: rust\n---\nLook at ownership."
        });
    }));
    assert!(message.contains("review"), "{message}");
}

#[test]
fn an_evaluation_never_reads_its_own_change_under_lenses() {
    let message = refused(&reading(|v| {
        v["completeness"] = json!({"skillDocument": "the skill"});
    }));
    assert!(message.contains("completeness"), "{message}");
}

#[test]
fn a_gap_step_never_reads_its_own_change_under_lenses() {
    let message = refused(&reading(|v| {
        v["gap"] = json!({"skillDocument": "the skill", "because": "not met"});
    }));
    assert!(message.contains("gap"), "{message}");
}
