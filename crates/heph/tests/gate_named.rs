//! Running a manifest's gate by name from the library, and wrapping a command
//! in the repository's toolchain: what `heph gate run` and the executor share.

mod common;

use std::collections::HashMap;

use common::Scratch;
use heph::gate::{NotRun, Outcome, run_named, wrap};
use heph::manifest::{Manifest, parse};

const MANIFEST: &str = "schema_version = 0\n\n\
    [toolchain]\nwrapper = [\"nix\", \"develop\", \"--command\"]\ninside_env = \"IN_NIX\"\n\n\
    [components.app]\nroot = \".\"\npaths = [\"**\"]\n\n\
    [gates.first]\nrun = [\"sh\", \"-c\", \"pwd > where\"]\ntimeout_seconds = 30\n\n\
    [gates.docs]\nrun = [\"sh\", \"-c\", \"touch docs-ran\"]\ntimeout_seconds = 30\nwhen_paths = [\"docs/**\"]\n\n\
    [gates.needy]\nrun = [\"sh\", \"-c\", \"touch needy-ran\"]\ntimeout_seconds = 30\n\n\
    [gates.needy.requires]\nHEPH_NAMED_SECRET = \"secret\"\nHEPH_NAMED_VAR = \"env\"\n";

fn manifest() -> Manifest {
    parse(MANIFEST).expect("a valid manifest")
}

/// A manifest whose gates run as themselves, without the toolchain wrapper,
/// which the tests would otherwise have to find on the machine.
fn unwrapped() -> Manifest {
    parse(&MANIFEST.replace(
        "[toolchain]\nwrapper = [\"nix\", \"develop\", \"--command\"]\ninside_env = \"IN_NIX\"\n\n",
        "",
    ))
    .expect("a valid manifest")
}

fn no_environment(_: &str) -> Option<String> {
    None
}

#[test]
fn an_unknown_gate_is_not_run_and_the_declared_ones_are_named() {
    let scratch = Scratch::new("named-unknown");

    let error = run_named(
        &unwrapped(),
        "missing",
        &[],
        &scratch.0,
        no_environment,
        |_| {},
    )
    .expect_err("an unknown gate");

    let NotRun::NoSuchGate { name, declared } = &error else {
        panic!("not NoSuchGate: {error:?}");
    };
    assert_eq!(name, "missing");
    assert_eq!(declared, &["docs", "first", "needy"]);
    assert!(error.to_string().contains("docs, first, needy"), "{error}");
}

#[test]
fn a_gate_whose_requirement_is_unset_is_not_run_and_its_command_does_not_run() {
    let scratch = Scratch::new("named-needy");
    let environment: HashMap<&str, &str> = HashMap::from([("HEPH_NAMED_VAR", "set")]);

    let error = run_named(
        &unwrapped(),
        "needy",
        &[],
        &scratch.0,
        |name| environment.get(name).map(|v| (*v).to_owned()),
        |_| {},
    )
    .expect_err("a missing requirement");

    let NotRun::Missing(missing) = &error else {
        panic!("not Missing: {error:?}");
    };
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].0, "HEPH_NAMED_SECRET");
    assert!(
        !scratch.exists("needy-ran"),
        "the command ran although a requirement was missing"
    );
}

#[test]
fn a_gate_that_applies_to_no_changed_file_is_skipped_and_one_that_does_runs() {
    let scratch = Scratch::new("named-applies");
    let changed = vec!["src/lib.rs".to_owned()];

    let skipped = run_named(
        &unwrapped(),
        "docs",
        &changed,
        &scratch.0,
        no_environment,
        |_| {},
    )
    .expect("a result");
    assert_eq!(skipped.outcome, Outcome::Skipped);
    assert!(!scratch.exists("docs-ran"));

    let ran = run_named(
        &unwrapped(),
        "docs",
        &["docs/a.md".to_owned()],
        &scratch.0,
        no_environment,
        |_| {},
    )
    .expect("a result");
    assert_eq!(ran.outcome, Outcome::Passed);
    assert!(scratch.exists("docs-ran"));
}

#[test]
fn a_gate_runs_in_the_directory_it_is_given() {
    let scratch = Scratch::new("named-directory");

    let result = run_named(
        &unwrapped(),
        "first",
        &[],
        &scratch.0,
        no_environment,
        |_| {},
    )
    .expect("a result");

    assert_eq!(result.outcome, Outcome::Passed);
    let where_ = scratch.read("where");
    assert_eq!(
        std::path::Path::new(where_.trim())
            .canonicalize()
            .expect("a path"),
        scratch.0.canonicalize().expect("a path")
    );
}

#[test]
fn the_toolchain_wrapper_goes_in_front_of_a_command_unless_the_caller_is_inside_it() {
    let command = vec!["npm".to_owned(), "ci".to_owned()];

    let outside = wrap(&manifest(), &command, no_environment);
    assert_eq!(outside, ["nix", "develop", "--command", "npm", "ci"]);

    let inside = wrap(&manifest(), &command, |name| {
        (name == "IN_NIX").then(|| "1".to_owned())
    });
    assert_eq!(inside, ["npm", "ci"]);

    let empty = wrap(&manifest(), &command, |name| {
        (name == "IN_NIX").then(String::new)
    });
    assert_eq!(
        empty,
        ["nix", "develop", "--command", "npm", "ci"],
        "an empty variable is not inside"
    );
}
