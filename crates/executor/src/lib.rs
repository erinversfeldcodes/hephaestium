//! The Hephaestium executor: runs one workload in a workspace of its own and
//! says everything it does as events.

pub mod adapter;
pub mod agent;
pub mod claude_code;
pub mod cli;
pub mod completeness;
pub mod deliver;
pub mod gap;
pub mod prepush;
pub mod provenance;
pub mod redact;
pub mod review;
pub mod run;
pub mod secrets;
pub mod stream;
pub mod toolenv;
pub mod transcript;
pub mod workload;
