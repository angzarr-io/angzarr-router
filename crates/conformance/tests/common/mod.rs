//! Running a harness's features: the router-local `conformance/` features
//! and angzarr-project's handler-contract tier
//! (`features/handler-contract/`, the router's complete contract, decision
//! 14.42), each against the tier's fixtures (`fixture/`).
//!
//! It runs whole: every scenario, none held out.

use std::path::PathBuf;

/// A path under the workspace root.
pub fn path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// A handler-contract tier feature, by file name.
pub fn tier(feature: &str) -> PathBuf {
    path("angzarr-project/features/handler-contract").join(feature)
}

/// Fails the harness when any run failed.
pub fn exit(failed: bool) {
    if failed {
        eprintln!("conformance: a feature failed");
        std::process::exit(1);
    }
}
