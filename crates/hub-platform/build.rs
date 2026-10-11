//! Compiles `proto/replica.proto` (the hub-to-hub RPC, private to this crate) with `protox`, a pure-Rust protobuf compiler,
//! so the build needs no `protoc` binary (Q10). The generated code is not committed.
//!
//! The messages of the contract (`HubMessage`, `AgentMessage`) are not generated again: `extern_path` points them at the
//! `proto` crate, so a forwarded command is the very type `proto::convert` validates.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let local = manifest.join("proto");
    let contract = manifest.join("../../proto");
    let file = local.join("replica.proto");
    println!("cargo:rerun-if-changed={}", file.display());
    println!(
        "cargo:rerun-if-changed={}",
        contract.join("agent.proto").display()
    );

    let descriptors = protox::compile([&file], [&local, &contract])?;
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .build_transport(false)
        .extern_path(".lanekeeper.agent.v1", "::proto::pb")
        .compile_fds(descriptors)?;
    Ok(())
}
