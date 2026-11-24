//! What the `heph` binary does, as a library, so that the binary and its
//! tests exercise the same code.

#[cfg(not(unix))]
compile_error!(
    "heph runs each gate in a process group of its own so that it can stop everything the gate \
     started, and so supports only Unix"
);

pub mod apply;
pub mod apply_settings;
pub mod baseline;
pub mod gate;
pub mod github;
pub mod lenses;
pub mod link;
pub mod lock;
pub mod manifest;
pub mod process;
pub mod run;
pub mod schema;
pub mod sync;
pub mod worker;
pub mod workload;
pub mod workspaces;

/// What this build is made of, fixed when it was compiled.
pub mod build {
    /// The commit the binary was built from; empty where the build could not tell.
    pub const COMMIT: &str = env!("HEPH_BUILD_COMMIT");
    /// The Athenaeum release the binary is linked against, as its tag.
    pub const ATHENAEUM_TAG: &str = env!("HEPH_ATHENAEUM_TAG");
    /// The commit that tag named when the lock resolved it; empty where it is not known.
    pub const ATHENAEUM_COMMIT: &str = env!("HEPH_ATHENAEUM_COMMIT");
}
