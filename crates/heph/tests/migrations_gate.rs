//! The repository's own `migrations` gate, as the manifest declares it: it must run when a
//! control-plane migration is changed and be skipped when none is. The script's behaviour is
//! tested by `scripts/test-check-migrations.sh`.

mod common;

use std::path::PathBuf;

use common::{Scratch, heph_in, stdout};
use serde_json::Value;

fn outcome_for(name: &str, changed: &str) -> String {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.athenaeum/manifest.toml");
    // An empty directory, so a gate that does run cannot touch the checkout; it fails there
    // for want of migrations, which is not a skip.
    let scratch = Scratch::new(name);
    let output = heph_in(
        &scratch.0,
        &[
            "gate",
            "run",
            "migrations",
            "--manifest",
            manifest.to_str().expect("a UTF-8 path"),
            "--changed",
            changed,
        ],
    );
    let result: Value = serde_json::from_str(stdout(&output).trim()).expect("a gate result");
    result["outcome"].as_str().expect("an outcome").to_owned()
}

#[test]
fn the_migrations_gate_is_skipped_when_no_migration_changed() {
    assert_eq!(
        outcome_for("migrations-skipped", "control-plane/lib/control_plane.ex"),
        "skipped"
    );
}

#[test]
fn the_migrations_gate_runs_when_a_migration_is_added() {
    assert_ne!(
        outcome_for(
            "migrations-runs",
            "control-plane/priv/repo/migrations/20261124130000_add_a_column.exs"
        ),
        "skipped"
    );
}
