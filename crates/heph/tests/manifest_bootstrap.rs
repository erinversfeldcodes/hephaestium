//! What `heph` reads of a manifest's bootstrap steps.

use heph::manifest::parse;

const BASE: &str = "schema_version = 0\n\n[components.app]\nroot = \".\"\npaths = [\"**\"]\n";

#[test]
fn the_steps_are_read_in_order_with_their_command_directory_and_timeout() {
    let manifest = parse(&format!(
        "{BASE}\n[[bootstrap]]\nrun = [\"npm\", \"ci\"]\nroot = \"web\"\ntimeout_seconds = 900\n\n\
         [[bootstrap]]\nrun = [\"just\", \"setup\"]\ntimeout_seconds = 60\n"
    ))
    .expect("a valid manifest");

    assert_eq!(manifest.bootstrap.len(), 2);
    assert_eq!(manifest.bootstrap[0].run, ["npm", "ci"]);
    assert_eq!(manifest.bootstrap[0].root.as_deref(), Some("web"));
    assert_eq!(manifest.bootstrap[0].timeout_seconds, 900);
    assert_eq!(manifest.bootstrap[1].run, ["just", "setup"]);
    assert_eq!(
        manifest.bootstrap[1].root, None,
        "no directory means the repository root"
    );
    assert_eq!(manifest.bootstrap[1].timeout_seconds, 60);
}

#[test]
fn a_manifest_with_no_bootstrap_has_none() {
    let manifest = parse(BASE).expect("a valid manifest");
    assert_eq!(manifest.bootstrap, []);
}

/// The repository's own manifest fetches what its tests need, so a fresh workspace can run one
/// control plane test and one dashboard test.
#[test]
fn this_repositorys_manifest_fetches_the_control_planes_and_the_dashboards_dependencies() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.athenaeum/manifest.toml"
    ))
    .expect("the repository's manifest");
    let manifest = parse(&text).expect("a valid manifest");

    let runs = |root: &str, words: &[&str]| {
        manifest
            .bootstrap
            .iter()
            .any(|step| step.root.as_deref() == Some(root) && step.run == words)
    };
    assert!(runs("control-plane", &["mix", "deps.get"]));
    assert!(runs("dashboard", &["npm", "ci", "--no-audit", "--no-fund"]));
}
