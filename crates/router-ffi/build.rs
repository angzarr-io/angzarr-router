use std::path::PathBuf;

fn main() {
    let repo_proto = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    println!("cargo:rerun-if-changed={}", repo_proto.display());

    prost_build::Config::new()
        .compile_protos(
            &[
                repo_proto.join("io/angzarr/router/ffi/v1/abi.proto"),
                repo_proto.join("google/rpc/status.proto"),
                repo_proto.join("google/rpc/error_details.proto"),
            ],
            // /usr/include supplies the protobuf well-known types
            // (google/protobuf/any.proto) that status.proto imports.
            &[repo_proto, PathBuf::from("/usr/include")],
        )
        .expect("prost: compile ABI protos");
}
