use std::path::PathBuf;

/// Compile the conformance fixture proto (`test.counter`) and emit a
/// FileDescriptorSet covering it and the framework protos it imports — the
/// pool the harness uses to parse `.txtpb` fixtures (textproto, with Any
/// expansion) into typed messages.
///
/// `io.angzarr.v1.*` and `sererr.v1.*` are extern-pathed to the router crate
/// so we DON'T regenerate them: the harness must build the SAME Rust types
/// `AggregateDispatch::dispatch` consumes. extern_path suppresses codegen
/// only — the descriptor set still carries those files for the pool.
fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let top = manifest.join("../..");
    let framework_proto = top.join("angzarr-project/proto");
    let conformance_proto = top.join("conformance/proto");
    let counter = conformance_proto.join("test/counter/counter.proto");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let fds_path = out_dir.join("conformance_fds.bin");

    println!("cargo:rerun-if-changed={}", conformance_proto.display());
    println!("cargo:rerun-if-changed={}", framework_proto.display());

    let mut config = prost_build::Config::new();
    config
        .extern_path(".io.angzarr.v1", "::angzarr_router::pb")
        .extern_path(".sererr.v1", "::angzarr_router::proto::sererr::v1")
        .file_descriptor_set_path(&fds_path)
        .compile_protos(&[counter], &[framework_proto, conformance_proto])
        .expect("prost: compile conformance protos");
}
