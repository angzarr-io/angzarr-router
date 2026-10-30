//! Cucumber harness for the handler-contract tier's
//! `projector_dispatch.feature`: drives it against the Rust router's
//! ProjectorDispatch. Every router binds the same feature; only this
//! step-definition layer differs.

mod common;

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::writer::Stats as _;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct ProjectorWorld {
    /// Outcome of the dispatched delivery.
    result: Option<Result<pb::Projection, CodedError>>,
    /// The projector's declared domains.
    domains: Vec<&'static str>,
}

impl ProjectorWorld {
    fn dispatch(&mut self, book: pb::EventBook) {
        self.result = Some(conf::counter_projector(&self.domains).dispatch(&book));
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
async fn a_counter_projection(w: &mut ProjectorWorld) {
    w.domains = vec!["counter"];
}

#[given("a counter projection over every domain")]
async fn a_counter_projection_over_every_domain(w: &mut ProjectorWorld) {
    w.domains = vec![angzarr_router::WILDCARD_DOMAIN];
}

#[when(regex = r#"^(\d+) events are delivered in domain "([^"]*)"$"#)]
async fn events_delivered(w: &mut ProjectorWorld, count: u32, domain: String) {
    w.dispatch(conf::delivery(&domain, count));
}

#[then(regex = r"^the projection records (\d+) events?$")]
async fn records_count(w: &mut ProjectorWorld, count: u64) {
    assert_eq!(
        w.projection().next_sequence,
        count,
        "every delivered event folds into one projection"
    );
}

#[then("the projection records nothing")]
async fn records_nothing(w: &mut ProjectorWorld) {
    assert_eq!(
        w.projection().next_sequence,
        0,
        "an undeclared domain folds nothing"
    );
}

#[tokio::main]
async fn main() {
    let tier = ProjectorWorld::cucumber()
        .fail_on_skipped()
        .run(common::tier("projector_dispatch.feature"))
        .await;
    common::exit(tier.execution_has_failed());
}
