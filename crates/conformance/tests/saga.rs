//! Cucumber harness for the saga behavior suite: drives the shared
//! `saga.feature` against the Rust core's SagaDispatch natively. The same
//! feature runs against the bindings and the generated clients in later
//! units — only this step-definition layer differs.

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct SagaWorld {
    /// Outcome of the dispatched source.
    result: Option<Result<pb::SagaResponse, CodedError>>,
}

impl SagaWorld {
    fn dispatch(&mut self, req: pb::SagaHandleRequest) {
        self.result = Some(conf::order_saga().dispatch(&req));
    }

    fn response(&self) -> &pb::SagaResponse {
        self.result
            .as_ref()
            .expect("a source was dispatched")
            .as_ref()
            .expect("expected a successful saga response")
    }
}

#[given(regex = r#"^an order saga delivering to "([^"]*)"$"#)]
async fn an_order_saga(_w: &mut SagaWorld, _target: String) {
    // Each dispatch builds a fresh saga; nothing to seed.
}

#[when(regex = r"^an Increased event is dispatched with destination inventory sequence (\d+)$")]
async fn increased_with_destination(w: &mut SagaWorld, seq: u32) {
    w.dispatch(conf::saga_event_source(
        "test.counter.Increased",
        &[("inventory", seq)],
    ));
}

#[when("a Reserve event is dispatched")]
async fn reserve_event(w: &mut SagaWorld) {
    w.dispatch(conf::saga_event_source("test.counter.Reserve", &[]));
}

#[when("a source with no pages is dispatched")]
async fn empty_source(w: &mut SagaWorld) {
    w.dispatch(conf::saga_empty_source());
}

#[when("a request with no source is dispatched")]
async fn missing_source(w: &mut SagaWorld) {
    w.dispatch(conf::saga_missing_source());
}

#[when("a rejection of Reserve is dispatched")]
async fn rejection_reserve(w: &mut SagaWorld) {
    w.dispatch(conf::saga_rejection_source("test.counter.Reserve"));
}

#[when("a rejection of Unwatched is dispatched")]
async fn rejection_unwatched(w: &mut SagaWorld) {
    w.dispatch(conf::saga_rejection_source("test.counter.Unwatched"));
}

#[then(regex = r#"^the saga emits one command to "([^"]*)"$"#)]
async fn emits_one_command(w: &mut SagaWorld, target: String) {
    let resp = w.response();
    assert_eq!(resp.commands.len(), 1, "exactly one command emitted");
    assert_eq!(
        resp.commands[0].cover.as_ref().expect("cover").domain,
        target
    );
}

#[then(regex = r"^the command carries destination sequence (\d+)$")]
async fn command_carries_sequence(w: &mut SagaWorld, seq: u32) {
    let cmd = &w.response().commands[0];
    let got = match cmd.pages[0]
        .header
        .as_ref()
        .and_then(|h| h.sequence_type.as_ref())
    {
        Some(pb::page_header::SequenceType::Sequence(s)) => *s,
        _ => panic!("command page carries no explicit sequence"),
    };
    assert_eq!(got, seq, "command stamped with the destination sequence");
}

#[then("the saga emits no commands")]
async fn emits_no_commands(w: &mut SagaWorld) {
    assert!(w.response().commands.is_empty(), "no commands emitted");
}

#[then("the saga injects one fact event")]
async fn injects_one_event(w: &mut SagaWorld) {
    assert_eq!(w.response().events.len(), 1, "one fact event injected");
}

#[then("the saga injects no events")]
async fn injects_no_events(w: &mut SagaWorld) {
    assert!(w.response().events.is_empty(), "no events injected");
}

#[then(regex = r"^the dispatch fails with ([A-Z_]+)$")]
async fn fails_with(w: &mut SagaWorld, code: String) {
    let err = w
        .result
        .as_ref()
        .expect("a source was dispatched")
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("expected failure {code}, got a success"));
    assert_eq!(err.code, code, "coded-error reason");
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/features/saga.feature"
    );
    SagaWorld::cucumber().run_and_exit(feature).await;
}
