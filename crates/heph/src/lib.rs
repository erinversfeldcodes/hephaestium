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
pub mod lock;
pub mod manifest;
pub mod process;
pub mod run;
pub mod schema;
pub mod sync;
pub mod workload;
