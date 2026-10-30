use std::path::PathBuf;

/// Compiles abi.proto to a FileDescriptorSet (OUT_DIR/abi_fds.bin): the
/// generator's one input. The Rust prost-build writes beside it is unused.
fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_proto = manifest.join("../../proto");
    let project_proto = match std::env::var("ANGZARR_PROJECT_PROTO") {
        Ok(root) => PathBuf::from(root),
        Err(_) => manifest.join("../../angzarr-project/proto"),
    };
    let wkt = match std::env::var("PROTOC_INCLUDE") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => PathBuf::from("/usr/include"),
    };
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    println!("cargo:rerun-if-env-changed=ANGZARR_PROJECT_PROTO");
    println!("cargo:rerun-if-env-changed=PROTOC_INCLUDE");
    println!("cargo:rerun-if-changed={}", repo_proto.display());
    prost_build::Config::new()
        .file_descriptor_set_path(out_dir.join("abi_fds.bin"))
        .compile_protos(
            &[repo_proto.join("io/angzarr/router/ffi/v1/abi.proto")],
            &[repo_proto, project_proto, wkt],
        )
        .expect("prost: compile abi.proto");
}
