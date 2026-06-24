use std::path::PathBuf;

fn main() {
    let repo_proto = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    // abi.proto imports io/angzarr/v1/types.proto (for Cover); the shared
    // framework protos live in the angzarr-project submodule.
    let project_proto = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../angzarr-project/proto");
    println!("cargo:rerun-if-changed={}", repo_proto.display());

    prost_build::Config::new()
        // Reuse the core crate's generated io.angzarr.v1 types (so SagaEventAux's
        // source_cover IS angzarr_router::pb::Cover — passed through unmolested,
        // no duplicate type) rather than regenerating them here.
        .extern_path(".io.angzarr.v1", "::angzarr_router::pb")
        .compile_protos(
            &[
                repo_proto.join("io/angzarr/router/ffi/v1/abi.proto"),
                repo_proto.join("google/rpc/status.proto"),
                repo_proto.join("google/rpc/error_details.proto"),
            ],
            // /usr/include supplies the protobuf well-known types
            // (google/protobuf/any.proto) that status.proto imports.
            &[repo_proto, project_proto, PathBuf::from("/usr/include")],
        )
        .expect("prost: compile ABI protos");
}
