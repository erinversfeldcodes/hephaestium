//! `heph lenses` as a person or the control plane meets it.
//!
//! It says which lenses a change needs, from the files the change touches and
//! the packs the manifest's components name. It prints one line of JSON.

mod common;

use std::process::Output;

use common::{Scratch, heph_in, stderr, stdout};
use serde_json::{Value, json};

fn component(name: &str, paths: &str, pack: Option<&str>) -> String {
    let pack = pack.map_or_else(String::new, |pack| format!("pack = {pack:?}\n"));
    format!("[components.{name}]\nroot = {name:?}\npaths = {paths}\n{pack}\n")
}

fn repository(name: &str, components: &str) -> Scratch {
    let scratch = Scratch::new(name);
    scratch.write(
        ".athenaeum/manifest.toml",
        &format!("schema_version = 0\n\n{components}"),
    );
    scratch
}

/// Rust in `core`, Elixir in `web`, and a component with no pack.
fn three_components() -> String {
    format!(
        "{}{}{}",
        component("core", "[\"core/**\"]", Some("rust")),
        component("web", "[\"web/**\"]", Some("elixir")),
        component("docs", "[\"docs/**\"]", None),
    )
}

fn lenses(scratch: &Scratch, changed: &[&str]) -> (Output, Value) {
    let mut arguments = vec!["lenses"];
    for path in changed {
        arguments.push("--changed");
        arguments.push(path);
    }
    let output = heph_in(&scratch.0, &arguments);
    let text = stdout(&output);
    let value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
    (output, value)
}

#[test]
fn a_change_to_two_components_needs_both_their_lenses_in_name_order() {
    let scratch = repository("two", &three_components());

    let (output, said) = lenses(&scratch, &["web/lib/a.ex", "core/src/lib.rs"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        said,
        json!({
            "lenses": [
                {"pack": "elixir", "components": ["web"]},
                {"pack": "rust", "components": ["core"]},
            ],
            "unpacked": [],
            "unowned": [],
        })
    );
}

#[test]
fn a_change_to_one_component_needs_only_its_lens() {
    let scratch = repository("one", &three_components());

    let (_, said) = lenses(&scratch, &["core/src/lib.rs"]);

    assert_eq!(
        said["lenses"],
        json!([{"pack": "rust", "components": ["core"]}])
    );
}

#[test]
fn two_components_on_one_pack_need_that_lens_once() {
    let components = format!(
        "{}{}",
        component("api", "[\"api/**\"]", Some("rust")),
        component("cli", "[\"cli/**\"]", Some("rust")),
    );
    let scratch = repository("shared", &components);

    let (_, said) = lenses(&scratch, &["cli/main.rs", "api/lib.rs"]);

    assert_eq!(
        said["lenses"],
        json!([{"pack": "rust", "components": ["api", "cli"]}])
    );
}

#[test]
fn a_component_that_names_no_pack_is_said_so_and_not_ignored() {
    let scratch = repository("unpacked", &three_components());

    let (_, said) = lenses(&scratch, &["docs/guide.md"]);

    assert_eq!(said["lenses"], json!([]));
    assert_eq!(said["unpacked"], json!(["docs"]));
}

#[test]
fn a_file_no_component_owns_is_said_so_and_not_ignored() {
    let scratch = repository("unowned", &three_components());

    let (_, said) = lenses(&scratch, &["core/src/lib.rs", "Makefile", "./scripts/x.sh"]);

    assert_eq!(
        said["lenses"],
        json!([{"pack": "rust", "components": ["core"]}])
    );
    assert_eq!(said["unowned"], json!(["Makefile", "./scripts/x.sh"]));
}

#[test]
fn a_leading_dot_slash_does_not_change_which_component_owns_a_file() {
    let scratch = repository("dotslash", &three_components());

    let (_, said) = lenses(&scratch, &["./web/lib/a.ex"]);

    assert_eq!(
        said["lenses"],
        json!([{"pack": "elixir", "components": ["web"]}])
    );
    assert_eq!(said["unowned"], json!([]));
}

#[test]
fn a_file_two_components_claim_is_attributed_to_both() {
    let components = format!(
        "{}{}",
        component("core", "[\"shared/**\"]", Some("rust")),
        component("web", "[\"shared/**\"]", Some("elixir")),
    );
    let scratch = repository("both", &components);

    let (_, said) = lenses(&scratch, &["shared/x"]);

    assert_eq!(
        said["lenses"],
        json!([
            {"pack": "elixir", "components": ["web"]},
            {"pack": "rust", "components": ["core"]},
        ])
    );
}

#[test]
fn naming_no_changed_file_is_a_usage_error_and_not_every_lens() {
    let scratch = repository("none", &three_components());

    let output = heph_in(&scratch.0, &["lenses"]);

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "", "it must print no verdict");
}

#[test]
fn a_manifest_that_cannot_be_read_is_not_a_change_with_no_lenses() {
    let scratch = Scratch::new("nomanifest");

    let output = heph_in(&scratch.0, &["lenses", "--changed", "core/a.rs"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "");
}

#[test]
fn a_glob_that_is_not_valid_is_could_not_run_and_not_no_lenses() {
    let components = component("core", "[\"core/[\"]", Some("rust"));
    let scratch = repository("badglob", &components);

    let output = heph_in(&scratch.0, &["lenses", "--changed", "core/a.rs"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "");
}

#[test]
fn the_manifest_can_be_named() {
    let scratch = Scratch::new("named");
    scratch.write(
        "elsewhere.toml",
        &format!("schema_version = 0\n\n{}", three_components()),
    );

    let output = heph_in(
        &scratch.0,
        &[
            "lenses",
            "--manifest",
            "elsewhere.toml",
            "--changed",
            "web/a.ex",
        ],
    );
    let said: Value = serde_json::from_str(stdout(&output).trim()).expect("JSON");

    assert_eq!(
        said["lenses"],
        json!([{"pack": "elixir", "components": ["web"]}])
    );
}
