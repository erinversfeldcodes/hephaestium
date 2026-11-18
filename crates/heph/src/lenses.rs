//! Which lenses a change needs.
//!
//! A change is attributed to components by their `paths`, the same way
//! `heph lint` reads it, and each component names the pack whose review lens
//! applies to it. What no lens will look at is said, never left out: a
//! component that names no pack, and a file no component owns.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::gate::{self, CouldNotRun};
use crate::manifest::Manifest;

/// One lens a change needs, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lens {
    /// The pack whose `review.md` is the lens.
    pub pack: String,
    /// The touched components that name the pack, in name order.
    pub components: Vec<String>,
}

/// What a change needs reviewed, and what nothing will review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// The lenses, in pack-name order.
    pub lenses: Vec<Lens>,
    /// Touched components that name no pack, so no lens applies to them.
    pub unpacked: Vec<String>,
    /// Changed files that no component owns, as they were given.
    pub unowned: Vec<String>,
}

/// The lenses a change to these files needs.
///
/// # Errors
///
/// Returns an error when a component's `paths` holds a glob that is not valid.
pub fn plan(manifest: &Manifest, changed: &[String]) -> Result<Plan, CouldNotRun> {
    let mut touched = BTreeSet::new();
    let mut unowned = Vec::new();
    for file in changed {
        let mut owned = false;
        for (name, component) in &manifest.components {
            if gate::matches_any(&component.paths, std::slice::from_ref(file))
                .map_err(|error| CouldNotRun::BadGlob(format!("component {name}: {error}")))?
            {
                owned = true;
                touched.insert(name.as_str());
            }
        }
        if !owned {
            unowned.push(file.clone());
        }
    }

    let mut by_pack: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut unpacked = Vec::new();
    for name in touched {
        match manifest.components[name].pack.as_deref() {
            Some(pack) => by_pack.entry(pack).or_default().push(name.to_owned()),
            None => unpacked.push(name.to_owned()),
        }
    }

    Ok(Plan {
        lenses: by_pack
            .into_iter()
            .map(|(pack, components)| Lens {
                pack: pack.to_owned(),
                components,
            })
            .collect(),
        unpacked,
        unowned,
    })
}
