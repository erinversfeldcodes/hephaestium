//! The runtime contracts as Rust types, generated from `proto/` and read and
//! written in the Protobuf JSON mapping, which is their wire form.

pub mod v1alpha1 {
    #![allow(clippy::pedantic, clippy::all, missing_docs)]
    include!(concat!(env!("OUT_DIR"), "/hephaestium.v1alpha1.rs"));
    include!(concat!(env!("OUT_DIR"), "/hephaestium.v1alpha1.serde.rs"));
}
