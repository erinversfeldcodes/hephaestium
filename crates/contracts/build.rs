//! Generates the contracts' types from the `.proto` files, so the contract has
//! one home and these types cannot drift from it. `protox` compiles the files in
//! Rust, which leaves `protoc` out of what a build needs.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("../../proto");
    let files: Vec<PathBuf> = std::fs::read_dir(root.join("hephaestium/v1alpha1"))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    println!("cargo:rerun-if-changed={}", root.display());

    let descriptors = protox::compile(&files, [&root])?;
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    let descriptor_path = out.join("descriptor.bin");
    std::fs::write(
        &descriptor_path,
        prost::Message::encode_to_vec(&descriptors),
    )?;

    prost_build::Config::new()
        .compile_well_known_types()
        .extern_path(".google.protobuf", "::pbjson_types")
        .bytes(["."])
        .skip_protoc_run()
        .file_descriptor_set_path(&descriptor_path)
        .compile_fds(descriptors.clone())?;

    pbjson_build::Builder::new()
        .register_descriptors(&prost::Message::encode_to_vec(&descriptors))?
        .build(&[".hephaestium.v1alpha1"])?;
    Ok(())
}
