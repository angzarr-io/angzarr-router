use std::path::PathBuf;

/// Resolves the angzarr-project proto root exactly as crates/router does: the
/// ANGZARR_PROJECT_PROTO override first, then the repo-local submodule.
fn project_proto_root() -> PathBuf {
    if let Ok(root) = std::env::var("ANGZARR_PROJECT_PROTO") {
        return PathBuf::from(root);
    }
    let submodule = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../angzarr-project/proto");
    if submodule.join("io/angzarr/v1/types.proto").exists() {
        return submodule;
    }
    panic!(
        "angzarr-project protos not found; set ANGZARR_PROJECT_PROTO or \
         init the angzarr-project submodule (git submodule update --init)"
    );
}

/// The directory holding the protobuf well-known types
/// (google/protobuf/any.proto, imported by google/rpc/status.proto):
/// PROTOC_INCLUDE when set, else the system protoc include directory.
fn well_known_types_root() -> PathBuf {
    match std::env::var("PROTOC_INCLUDE") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => PathBuf::from("/usr/include"),
    }
}

fn main() {
    let repo_proto = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    // abi.proto imports io/angzarr/v1/types.proto (for Cover); the shared
    // framework protos live in the angzarr-project submodule.
    let project_proto = project_proto_root();
    let wkt = well_known_types_root();
    println!("cargo:rerun-if-env-changed=ANGZARR_PROJECT_PROTO");
    println!("cargo:rerun-if-env-changed=PROTOC_INCLUDE");
    println!("cargo:rerun-if-changed={}", repo_proto.display());
    println!("cargo:rerun-if-changed={}", project_proto.display());

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
            &[repo_proto, project_proto, wkt],
        )
        .expect("prost: compile ABI protos");
}
