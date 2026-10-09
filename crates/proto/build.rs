//! Compiles `proto/agent.proto` with `protox` (a pure-Rust protobuf compiler), so the build needs no
//! `protoc` binary (Q10). The generated code is not committed.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("../../proto");
    let file = root.join("agent.proto");
    println!("cargo:rerun-if-changed={}", file.display());

    let descriptors = protox::compile([&file], [&root])?;
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        // The generated `connect(dst)` helper would clash with the `Connect` RPC. Clients are built with
        // `grpc::agent_client` from any channel instead.
        .build_transport(false)
        // File bytes stay `Bytes` end to end, so a scan delta is never copied into a `Vec<u8>` (rust conventions).
        .bytes(".")
        // The join request carries tokens (S7, S21): its `Debug` is written by hand in `lib.rs` and redacts them.
        .skip_debug([".lanekeeper.agent.v1.JoinRequest"])
        .compile_fds(descriptors)?;
    Ok(())
}
