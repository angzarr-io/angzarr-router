//! Cucumber harness for the policy behavior suite: drives the shared
//! `policy.feature` against the Rust core's PolicyDispatch natively. The same
//! feature runs against the bindings and the generated clients in later
//! units — only this step-definition layer differs.

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct PolicyWorld {
    /// Outcome of the dispatched source.
    result: Option<Result<pb::PolicyResponse, CodedError>>,
    /// The source sequences the policy handler saw.
    seen: conf::SequenceSink,
}

impl PolicyWorld {
    fn dispatch(&mut self, req: pb::EventBook) {
        self.result = Some(conf::order_policy(self.seen.clone()).dispatch(&req));
    }

    fn response(&self) -> &pb::PolicyResponse {
        self.result
            .as_ref()
            .expect("a source was dispatched")
            .as_ref()
            .expect("expected a successful policy response")
    }
}

#[given(regex = r#"^an order policy delivering to "([^"]*)"$"#)]
async fn an_order_policy(_w: &mut PolicyWorld, _target: String) {
    // Each dispatch builds a fresh policy; nothing to seed.
}

#[when(regex = r"^an Increased event at sequence (\d+) is dispatched$")]
async fn increased_at(w: &mut PolicyWorld, seq: u64) {
    w.dispatch(conf::policy_event_source("test.counter.Increased", seq));
}

#[when("a Reserve event is dispatched")]
async fn reserve_event(w: &mut PolicyWorld) {
    w.dispatch(conf::policy_event_source("test.counter.Reserve", 0));
}

#[when("a source with no pages is dispatched")]
async fn empty_source(w: &mut PolicyWorld) {
    w.dispatch(conf::policy_empty_source());
}

#[then(regex = r#"^the policy emits one command to "([^"]*)"$"#)]
async fn emits_one_command(w: &mut PolicyWorld, target: String) {
    let resp = w.response();
    assert_eq!(resp.commands.len(), 1, "exactly one command emitted");
    assert_eq!(
        resp.commands[0].cover.as_ref().expect("cover").domain,
        target
    );
}

#[then(regex = r"^the policy handler saw source sequence (\d+)$")]
async fn handler_saw_sequence(w: &mut PolicyWorld, seq: u64) {
    assert_eq!(*w.seen.lock().unwrap(), vec![seq]);
}

#[then("the command carries no router-stamped provenance")]
async fn no_stamped_provenance(w: &mut PolicyWorld) {
    let page = w.response().commands[0]
        .page
        .as_ref()
        .expect("a command page");
    assert_eq!(
        page.command_id, None,
        "the coordinator stamps the command id"
    );
    assert_eq!(
        page.correlation_id, None,
        "the coordinator stamps the correlation"
    );
    assert_eq!(
        page.causation_id, None,
        "the coordinator stamps the causation"
    );
}

#[then("the policy emits no commands")]
async fn emits_no_commands(w: &mut PolicyWorld) {
    assert!(w.response().commands.is_empty(), "no commands emitted");
}

#[then(regex = r"^the dispatch fails with ([A-Z_]+)$")]
async fn fails_with(w: &mut PolicyWorld, code: String) {
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
        "/../../conformance/features/policy.feature"
    );
    PolicyWorld::cucumber().run_and_exit(feature).await;
}
