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
    /// The source sequences the saga handler saw.
    seen: conf::SequenceSink,
}

impl SagaWorld {
    fn dispatch(&mut self, req: pb::SagaHandleRequest) {
        self.result = Some(conf::order_saga(self.seen.clone()).dispatch(&req));
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

#[when(regex = r"^an Increased event at sequence (\d+) is dispatched$")]
async fn increased_at(w: &mut SagaWorld, seq: u32) {
    w.dispatch(conf::saga_event_source("test.counter.Increased", Some(seq)));
}

#[when("a Reserve event is dispatched")]
async fn reserve_event(w: &mut SagaWorld) {
    w.dispatch(conf::saga_event_source("test.counter.Reserve", None));
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

#[then(regex = r#"^the saga emits one command to "([^"]*)"$"#)]
async fn emits_one_command(w: &mut SagaWorld, target: String) {
    let resp = w.response();
    assert_eq!(resp.commands.len(), 1, "exactly one command emitted");
    assert_eq!(
        resp.commands[0].cover.as_ref().expect("cover").domain,
        target
    );
}

#[then(regex = r"^the command is deferred from source sequence (\d+) at command index (\d+)$")]
async fn command_is_deferred(w: &mut SagaWorld, seq: u32, index: u32) {
    let cmd = &w.response().commands[0];
    for page in &cmd.pages {
        let Some(pb::page_header::SequenceType::AngzarrDeferred(d)) =
            page.header.as_ref().and_then(|h| h.sequence_type.as_ref())
        else {
            panic!("command page is not deferred");
        };
        assert_eq!(d.source_seq, seq, "source_seq is the triggering event's");
        assert_eq!(
            d.command_index, index,
            "command_index is the emission position"
        );
        assert_eq!(
            d.source.as_ref().map(|c| c.domain.as_str()),
            Some("order"),
            "the source cover is the triggering book's"
        );
    }
}

#[then(regex = r"^the saga handler saw source sequence (\d+)$")]
async fn handler_saw_sequence(w: &mut SagaWorld, seq: u32) {
    assert_eq!(*w.seen.lock().unwrap(), vec![seq]);
}

#[then("the saga emits no commands")]
async fn emits_no_commands(w: &mut SagaWorld) {
    assert!(w.response().commands.is_empty(), "no commands emitted");
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
