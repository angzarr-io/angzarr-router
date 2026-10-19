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
    /// True for the parity saga.
    parity: bool,
}

impl SagaWorld {
    fn dispatch(&mut self, req: pb::SagaHandleRequest) {
        let saga = if self.parity {
            conf::parity_saga()
        } else {
            conf::order_saga(self.seen.clone())
        };
        self.result = Some(saga.dispatch(&req));
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

fn deferred(page: &pb::CommandPage) -> &pb::AngzarrDeferredSequence {
    match page.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
        Some(pb::page_header::SequenceType::AngzarrDeferred(d)) => d,
        other => panic!("command page is not deferred: {other:?}"),
    }
}

#[then("the command leaves its source component to the coordinator")]
async fn no_source_component(w: &mut SagaWorld) {
    for page in &w.response().commands[0].pages {
        assert_eq!(
            deferred(page).source_component,
            "",
            "the coordinator stamps the component"
        );
    }
}

#[when(regex = r#"^an Increased event of order root "([^"]*)" at sequence (\d+) is dispatched$"#)]
async fn rooted_increased(w: &mut SagaWorld, label: String, seq: u32) {
    w.dispatch(conf::saga_rooted_source(&label, seq));
}

#[then(regex = r#"^the command is deferred from order root "([^"]*)"$"#)]
async fn deferred_from_root(w: &mut SagaWorld, label: String) {
    let resp = w.response();
    assert_eq!(resp.commands.len(), 1);
    for page in &resp.commands[0].pages {
        assert_eq!(
            deferred(page).source.as_ref(),
            Some(&conf::cover_of("order", &label)),
            "the source is the triggering book's whole cover"
        );
    }
}

#[given("a parity saga emitting the parity command twice")]
async fn a_parity_saga(w: &mut SagaWorld) {
    w.parity = true;
}

#[when(regex = r"^the parity source event at sequence (\d+) is dispatched$")]
async fn parity_source(w: &mut SagaWorld, seq: u32) {
    w.dispatch(conf::parity_source(seq));
}

#[then(regex = r#"^the command at index (\d+) hashes to SHA-256 "([0-9a-f]{64})"$"#)]
async fn command_hash(w: &mut SagaWorld, index: usize, hash: String) {
    use prost::Message;
    use sha2::{Digest, Sha256};
    let cmd = &w.response().commands[index];
    assert_eq!(format!("{:x}", Sha256::digest(cmd.encode_to_vec())), hash);
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
