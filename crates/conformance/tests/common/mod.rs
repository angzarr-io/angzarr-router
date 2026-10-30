//! Running a harness's features: the router-local `conformance/` features
//! and angzarr-project's handler-contract tier
//! (`features/handler-contract/`, the router's complete contract, decision
//! 14.42), each against the tier's fixtures (`fixture/`).
//!
//! The tier already asserts the router's later shape: no upcasting (14.39),
//! no covered-page guard (14.40), and no admission, policy checks, refusal
//! building or conflict judgement (14.42). Until those removals land, the
//! scenarios asserting them are held out of the run by ID, in
//! [`LATER_STEPS`], each with the removal that turns it green. A removal
//! step deletes its rows and runs them.

use std::path::PathBuf;

use cucumber::gherkin;

/// Tier scenarios held out until a later removal step: (scenario ID, the
/// removal they wait on).
pub const LATER_STEPS: &[(&str, &str)] = &[
    ("C-0543", "conflict judgement moves out (14.42)"),
    ("C-0544", "conflict judgement moves out (14.42)"),
    ("C-0545", "conflict judgement moves out (14.42)"),
    ("C-0547", "conflict judgement moves out (14.42)"),
    ("C-0550", "conflict judgement moves out (14.42)"),
    (
        "C-0692",
        "refusal building moves to the coordinator (14.42)",
    ),
];

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

/// True unless the scenario is held out for a later step.
pub fn runs(scenario: &gherkin::Scenario) -> bool {
    !scenario
        .tags
        .iter()
        .any(|tag| LATER_STEPS.iter().any(|(id, _)| tag == id))
}

/// Fails the harness when any run failed.
pub fn exit(failed: bool) {
    if failed {
        eprintln!("conformance: a feature failed");
        std::process::exit(1);
    }
}
