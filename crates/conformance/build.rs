use std::path::PathBuf;

/// Compile the conformance fixture proto (`test.counter`) and emit a
/// FileDescriptorSet covering it and the framework protos it imports — the
/// pool the harness uses to parse `.txtpb` fixtures (textproto, with Any
/// expansion) into typed messages.
///
/// `io.angzarr.v1.*` and `sererr.v1.*` are extern-pathed to the router crate
/// so we DON'T regenerate them: the harness must build the SAME Rust types
/// `DomainDispatch::dispatch` consumes. extern_path suppresses codegen
/// only — the descriptor set still carries those files for the pool.
fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let top = manifest.join("../..");
    let framework_proto = top.join("angzarr-project/proto");
    // The handler-contract tier's fixtures (decision 14.42): the router's
    // complete contract, shared by every router.
    let fixture = top.join("angzarr-project/features/handler-contract/fixture");
    let counter = fixture.join("counter.proto");
    let order = fixture.join("order.proto");
    // The fixture pool parses framework envelopes (ContextualCommand, EventPage)
    // from the .txtpb fixtures. The message-annotation counter.proto no longer
    // imports these, so the harness names them explicitly — its descriptor
    // needs are its own, not an artifact of what the fixture proto imports.
    let types = framework_proto.join("io/angzarr/v1/types.proto");
    let command_handler = framework_proto.join("io/angzarr/v1/command_handler.proto");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let fds_path = out_dir.join("conformance_fds.bin");

    println!("cargo:rerun-if-changed={}", fixture.display());
    println!("cargo:rerun-if-changed={}", framework_proto.display());

    let mut config = prost_build::Config::new();
    config
        .enable_type_names()
        .extern_path(".io.angzarr.v1", "::angzarr_router::pb")
        .extern_path(".sererr.v1", "::angzarr_router::proto::sererr::v1")
        .file_descriptor_set_path(&fds_path)
        .compile_protos(
            &[counter, order, types, command_handler],
            &[framework_proto, fixture],
        )
        .expect("prost: compile conformance protos");
}
