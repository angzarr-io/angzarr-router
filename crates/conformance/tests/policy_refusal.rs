//! Binds angzarr-project's coordinator-contract
//! `features/coordinator-contract/policy_refusal.feature`, C-0617 only, to
//! the Rust core: the router's part of a refused policy command is the
//! class its answer's gRPC status decides (decision 14.37, owner rulings
//! of 2026-10-29). A business refusal reaches the policy's OnRefused (the
//! answer has a `RefusalResponse`); a bug is alerted and dead-lettered; the
//! rest are retried. Acting on the class (the retry, its cap, the dead
//! letter, the alert, building the refusal fact) is the runtime's.

use angzarr_router::error::CodedError;
use cucumber::World;

#[derive(Debug, Default, World)]
struct RefusalWorld {
    /// The wallet's answer to HoldFunds, as the router carries it.
    answer: Option<CodedError>,
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../angzarr-project/features/coordinator-contract/policy_refusal.feature"
    );
    RefusalWorld::cucumber()
        .fail_on_skipped()
        .filter_run_and_exit(feature, |_feature, _rule, scenario| {
            scenario.tags.iter().any(|tag| tag == "C-0617")
        })
        .await;
}
