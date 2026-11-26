//! What `heph` reads of a component's `test_changed` command.

use heph::manifest::parse;

const BASE: &str = "schema_version = 0\n\n[components.app]\nroot = \".\"\npaths = [\"**\"]\n";

#[test]
fn a_components_test_changed_command_is_read() {
    let manifest = parse(&format!(
        "{BASE}test_changed = [\"npx\", \"vitest\", \"related\", \"--run\"]\n"
    ))
    .expect("a valid manifest");

    assert_eq!(
        manifest.components["app"].test_changed.as_deref(),
        Some(&["npx", "vitest", "related", "--run"].map(String::from)[..])
    );
}

#[test]
fn a_component_with_no_test_changed_has_none() {
    let manifest = parse(BASE).expect("a valid manifest");
    assert_eq!(manifest.components["app"].test_changed, None);
}

#[test]
fn this_repositorys_manifest_is_still_accepted() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.athenaeum/manifest.toml"
    ))
    .expect("the repository's manifest");
    parse(&text).expect("a valid manifest");
}
