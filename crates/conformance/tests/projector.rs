//! Cucumber harness for the projector behavior suite: drives the shared
//! `projector.feature` against the Rust core's ProjectorDispatch natively.
//! The same feature runs against the bindings and the generated clients in
//! later units — only this step-definition layer differs.

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct ProjectorWorld {
    /// Outcome of the dispatched delivery.
    result: Option<Result<pb::Projection, CodedError>>,
}

impl ProjectorWorld {
    fn dispatch(&mut self, book: pb::EventBook) {
        self.result = Some(conf::counter_projector().dispatch(&book));
    }

    fn projection(&self) -> &pb::Projection {
        self.result
            .as_ref()
            .expect("a delivery was dispatched")
            .as_ref()
            .expect("expected a successful projection")
    }
}

#[given("a counter projection")]
async fn a_counter_projection(_w: &mut ProjectorWorld) {
    // Each dispatch builds a fresh projector; nothing to seed.
}

#[when(regex = r#"^(\d+) events are delivered in domain "([^"]*)"$"#)]
async fn events_delivered(w: &mut ProjectorWorld, count: u32, domain: String) {
    w.dispatch(conf::delivery(&domain, count));
}

#[when("a delivery arrives with no cover")]
async fn delivery_no_cover(w: &mut ProjectorWorld) {
    w.dispatch(conf::delivery_without_cover(1));
}

#[then(regex = r"^the projection records (\d+) events?$")]
async fn records_count(w: &mut ProjectorWorld, count: u32) {
    assert_eq!(
        w.projection().sequence,
        count,
        "every delivered event folds into one projection"
    );
}

#[then("the projection records nothing")]
async fn records_nothing(w: &mut ProjectorWorld) {
    assert_eq!(
        w.projection().sequence,
        0,
        "an undeclared domain folds nothing"
    );
}

#[then(regex = r"^the delivery fails with ([A-Z_]+)$")]
async fn fails_with(w: &mut ProjectorWorld, code: String) {
    let err = w
        .result
        .as_ref()
        .expect("a delivery was dispatched")
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("expected failure {code}, got a success"));
    assert_eq!(err.code, code, "coded-error reason");
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/features/projector.feature"
    );
    ProjectorWorld::cucumber().run_and_exit(feature).await;
}
