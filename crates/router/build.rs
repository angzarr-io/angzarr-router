//! The io.angzarr.v1 wire types come from angzarr-project's angzarr-proto
//! crate (types.proto, upcaster.proto, options.proto and sererr). That crate
//! does not compile the service protos, so the few request/response messages
//! they define (CommandHandlerResponse, CommandResponse, SagaResponse,
//! Refusal, OnRefusedResponse) are generated here, with every type
//! angzarr-proto provides extern-pathed to it: no io.angzarr.v1 type exists
//! twice.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use heck::ToUpperCamelCase;

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

/// The protos angzarr-proto compiles: their types are its, never ours.
const ANGZARR_PROTO_FILES: [&str; 3] = [
    "io/angzarr/v1/types.proto",
    "io/angzarr/v1/upcaster.proto",
    "io/angzarr/v1/options.proto",
];

/// The service protos whose messages this crate generates.
/// (process_manager.proto defines nothing since decision 14.31: a process
/// manager is a domain, served by command_handler.proto.)
const SERVICE_FILES: [&str; 2] = [
    "io/angzarr/v1/command_handler.proto",
    "io/angzarr/v1/saga.proto",
];

fn main() {
    let root = proto_root();
    println!("cargo:rerun-if-env-changed=ANGZARR_PROJECT_PROTO");
    println!("cargo:rerun-if-env-changed=PROTOC");
    for file in ANGZARR_PROTO_FILES.iter().chain(SERVICE_FILES.iter()) {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }

    let services: Vec<PathBuf> = SERVICE_FILES.iter().map(|f| root.join(f)).collect();
    let mut config = prost_build::Config::new();
    let fds = config
        .load_fds(&services, &[&root])
        .expect("prost: load the service protos");

    config
        .enable_type_names()
        .extern_path(".sererr.v1", "::angzarr_proto::sererr::v1");
    for name in angzarr_proto_types(&fds) {
        config.extern_path(
            format!(".io.angzarr.v1.{name}"),
            format!("::angzarr_proto::v1::{}", name.to_upper_camel_case()),
        );
    }
    config
        .compile_fds(fds)
        .expect("prost: compile the service messages");
}

/// The top-level message and enum names defined in angzarr-proto's files.
fn angzarr_proto_types(fds: &prost_types::FileDescriptorSet) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for file in &fds.file {
        let is_angzarr_proto = ANGZARR_PROTO_FILES
            .iter()
            .any(|f| Path::new(file.name()) == Path::new(f));
        if is_angzarr_proto {
            names.extend(file.message_type.iter().map(|m| m.name().to_string()));
            names.extend(file.enum_type.iter().map(|e| e.name().to_string()));
        }
    }
    names
}
