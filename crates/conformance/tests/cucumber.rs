//! Cucumber harness: drives the shared `.feature` behavior suite against the
//! Rust core natively. Step defs parse the orthogonal `.txtpb` skeletons,
//! set the scenario's data by field, dispatch, and assert outcomes.
//!
//! The same `.feature` files run against the bindings and the generated
//! clients in later units — only the step-definition layer differs.

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct CounterWorld {
    /// Prior-events book supplied to the next dispatch.
    prior: Option<pb::EventBook>,
    /// Outcome of the dispatched command.
    result: Option<Result<pb::BusinessResponse, CodedError>>,
}

impl CounterWorld {
    fn dispatch(&mut self, mut cc: pb::ContextualCommand) {
        cc.events = self.prior.clone();
        self.result = Some(conf::counter_aggregate().dispatch(&cc));
    }
}

#[given("a new counter")]
async fn new_counter(w: &mut CounterWorld) {
    w.prior = None;
}

#[given(regex = r"^a counter that has already recorded (\d+) increase$")]
async fn prior_increases(w: &mut CounterWorld, n: u32) {
    w.prior = conf::prior_history(n);
}

#[when(regex = r"^the operator increases the counter by (\d+)$")]
async fn increase_by(w: &mut CounterWorld, n: u32) {
    w.dispatch(conf::increase_command(n));
}

#[when("the operator triggers a hard failure")]
async fn hard_failure(w: &mut CounterWorld) {
    w.dispatch(conf::failhard_command());
}

#[when("an unhandled command is dispatched")]
async fn unhandled_command(w: &mut CounterWorld) {
    w.dispatch(conf::unhandled_command());
}

#[then(regex = r"^(\d+) increases are recorded, starting at sequence (\d+)$")]
async fn recorded_starting(w: &mut CounterWorld, count: u32, start: u32) {
    assert_recorded(w, count, start);
}

#[then(regex = r"^(\d+) increases are recorded, continuing from sequence (\d+)$")]
async fn recorded_continuing(w: &mut CounterWorld, count: u32, start: u32) {
    assert_recorded(w, count, start);
}

#[then(regex = r"^the command is rejected as ([A-Z_]+)$")]
async fn rejected_as(w: &mut CounterWorld, code: String) {
    assert_failed_with(w, &code);
}

#[then(regex = r"^the command fails with ([A-Z_]+)$")]
async fn fails_with(w: &mut CounterWorld, code: String) {
    assert_failed_with(w, &code);
}

#[then("no events are recorded")]
async fn no_events(w: &mut CounterWorld) {
    // A rejection/failure is an Err and carries no events; if some impl
    // returned Ok, it must carry an empty book.
    if let Some(Ok(resp)) = &w.result {
        if let Some(pb::business_response::Result::Events(book)) = &resp.result {
            assert!(
                book.pages.is_empty(),
                "expected no events recorded, got {}",
                book.pages.len()
            );
        }
    }
}

fn assert_recorded(w: &CounterWorld, count: u32, start: u32) {
    let resp = w
        .result
        .as_ref()
        .expect("a command was dispatched")
        .as_ref()
        .expect("expected a successful response");
    let book = match &resp.result {
        Some(pb::business_response::Result::Events(book)) => book,
        other => panic!("expected an events response, got {other:?}"),
    };
    assert_eq!(book.pages.len() as u32, count, "recorded event count");
    for (i, page) in book.pages.iter().enumerate() {
        assert_eq!(
            angzarr_router::page_sequence(page),
            start + i as u32,
            "sequence of recorded event {i}"
        );
    }
}

fn assert_failed_with(w: &CounterWorld, code: &str) {
    let err = w
        .result
        .as_ref()
        .expect("a command was dispatched")
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("expected failure {code}, got a success"));
    assert_eq!(err.code, code, "coded-error reason");
}

#[tokio::main]
async fn main() {
    let features = concat!(env!("CARGO_MANIFEST_DIR"), "/../../conformance/features");
    CounterWorld::cucumber().run_and_exit(features).await;
}
