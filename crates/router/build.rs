use std::path::PathBuf;

/// Resolves the angzarr-project proto root: env override first, then the
/// repo-local submodule.
fn proto_root() -> PathBuf {
    if let Ok(root) = std::env::var("ANGZARR_PROJECT_PROTO") {
        return PathBuf::from(root);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let submodule = manifest.join("../../angzarr-project/proto");
    if submodule.join("io/angzarr/v1/types.proto").exists() {
        return submodule;
    }
    panic!(
        "angzarr-project protos not found; set ANGZARR_PROJECT_PROTO or \
         init the angzarr-project submodule (git submodule update --init)"
    );
}

fn main() {
    let root = proto_root();
    println!("cargo:rerun-if-env-changed=ANGZARR_PROJECT_PROTO");
    println!("cargo:rerun-if-changed={}", root.display());

    prost_build::Config::new()
        .enable_type_names()
        .compile_protos(
            &[
                root.join("io/angzarr/v1/types.proto"),
                root.join("io/angzarr/v1/command_handler.proto"),
            ],
            &[root],
        )
        .expect("prost: compile framework protos");
}
