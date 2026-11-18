//! What a review workload must be to be read at all.
//!
//! A review reads one pull request at one commit under one lens. The executor
//! refuses one that is not safe to use before it creates anything, so these
//! test the strict reader that every run starts with.

mod common;

use common::workload;
use executor::workload::read;
use serde_json::{Value, json};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn review() -> Value {
    json!({
        "pullRequestNumber": "52",
        "pullRequestUrl": "https://github.com/erinversfeldcodes/gazebolang/pull/52",
        "lens": "rust",
        "lensDocument": "---\nname: rust\n---\nLook at ownership."
    })
}

fn reviewing(edit: impl FnOnce(&mut Value)) -> Value {
    let mut value = workload(COMMIT);
    value["review"] = review();
    edit(&mut value);
    value
}

fn refused(value: &Value) -> String {
    read(&value.to_string()).expect_err("it should be refused")
}

#[test]
fn a_review_of_a_pull_request_under_a_lens_is_read() {
    let workload = read(&reviewing(|_| {}).to_string()).expect("a review is a workload");

    let review = workload.review.expect("the review is kept");
    assert_eq!(review.pull_request_number, 52);
    assert_eq!(review.lens, "rust");
    assert!(review.lens_document.contains("ownership"));
}

#[test]
fn a_workload_with_no_review_has_none() {
    let workload = read(&workload(COMMIT).to_string()).expect("a workload");
    assert!(workload.review.is_none());
}

#[test]
fn a_workload_is_not_a_revision_and_a_review_at_once() {
    let value = reviewing(|v| {
        v["revision"] = json!({
            "pullRequestNumber": "52",
            "pullRequestUrl": "https://github.com/erinversfeldcodes/gazebolang/pull/52",
            "branch": "heph/12-x",
            "attempt": 1,
            "failures": [{"name": "test", "result": "failure", "excerpt": "x"}]
        });
    });

    let why = refused(&value);

    assert!(why.contains("revision") && why.contains("review"), "{why}");
}

#[test]
fn a_review_that_is_not_safe_is_refused() {
    let cases: [(&str, &str, Value); 9] = [
        ("a pull request number of zero", "pullRequestNumber", json!("0")),
        ("no pull request number", "pullRequestNumber", json!("")),
        ("a pull request that is not on GitHub", "pullRequestUrl", json!("file:///etc/passwd")),
        ("no lens", "lens", json!("")),
        ("a lens that climbs out of the packs", "lens", json!("../skills")),
        ("a lens with a slash", "lens", json!("a/b")),
        ("a lens that is an option", "lens", json!("-rust")),
        ("a lens with a space", "lens", json!("test truth")),
        ("no lens text", "lensDocument", json!("")),
    ];
    for (name, key, edit) in cases {
        let value = reviewing(|v| v["review"][key] = edit);
        let why = refused(&value);
        assert!(why.contains("review"), "{name}: {why}");
    }
}

#[test]
fn the_lens_names_a_pack_so_letters_digits_dashes_and_underscores_are_allowed() {
    for lens in ["rust", "test-truth", "elixir_2", "A1"] {
        let value = reviewing(|v| v["review"]["lens"] = json!(lens));
        assert!(read(&value.to_string()).is_ok(), "{lens} should be read");
    }
}

#[test]
fn a_misspelt_field_in_a_review_is_refused_and_not_ignored() {
    let value = reviewing(|v| v["review"]["lense"] = json!("rust"));
    assert!(read(&value.to_string()).is_err());
}
