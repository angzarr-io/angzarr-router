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

#[when("an Increased event is dispatched")]
async fn increased(w: &mut SagaWorld) {
    w.dispatch(conf::saga_event_source("test.counter.Increased"));
}

#[when("a Reserve event is dispatched")]
async fn reserve_event(w: &mut SagaWorld) {
    w.dispatch(conf::saga_event_source("test.counter.Reserve"));
}

#[when("a source with no pages is dispatched")]
async fn empty_source(w: &mut SagaWorld) {
    w.dispatch(conf::saga_empty_source());
}

#[when("a request with no source is dispatched")]
async fn missing_source(w: &mut SagaWorld) {
    w.dispatch(conf::saga_missing_source());
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

#[then("the command carries no router-stamped sequence")]
async fn command_unstamped(w: &mut SagaWorld) {
    // The coordinator stamps command sequences; the router does not.
    let cmd = &w.response().commands[0];
    assert!(
        cmd.pages[0].header.is_none(),
        "command page carries a header"
    );
}

#[then("the saga emits no commands")]
async fn emits_no_commands(w: &mut SagaWorld) {
    assert!(w.response().commands.is_empty(), "no commands emitted");
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
