//! Cucumber harness for the process-manager behavior suite: drives the shared
//! `process_manager.feature` against the Rust core's ProcessManagerDispatch
//! natively. The same feature runs against the bindings and the generated
//! clients in later units — only this step-definition layer differs.

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct ProcessManagerWorld {
    /// Outcome of the dispatched trigger.
    result: Option<Result<pb::ProcessManagerHandleResponse, CodedError>>,
}

impl ProcessManagerWorld {
    fn dispatch(&mut self, req: pb::ProcessManagerHandleRequest) {
        self.result = Some(conf::order_pm().dispatch(&req));
    }

    fn response(&self) -> &pb::ProcessManagerHandleResponse {
        self.result
            .as_ref()
            .expect("a trigger was dispatched")
            .as_ref()
            .expect("expected a successful PM response")
    }
}

#[given("an order process-manager")]
async fn an_order_pm(_w: &mut ProcessManagerWorld) {
    // Each dispatch builds a fresh PM; nothing to seed.
}

#[when(regex = r#"^an Increased trigger in domain "([^"]*)" is dispatched with destination inventory sequence (\d+)$"#)]
async fn increased_with_destination(w: &mut ProcessManagerWorld, domain: String, seq: u32) {
    w.dispatch(conf::pm_trigger_request(
        &domain,
        &["test.counter.Increased"],
        None,
        &[("inventory", seq)],
    ));
}

#[when(regex = r#"^an Increased trigger in domain "([^"]*)" is dispatched$"#)]
async fn increased_in_domain(w: &mut ProcessManagerWorld, domain: String) {
    w.dispatch(conf::pm_trigger_request(
        &domain,
        &["test.counter.Increased"],
        None,
        &[],
    ));
}

#[when("a trigger whose newest page is an undeclared event is dispatched")]
async fn newest_undeclared(w: &mut ProcessManagerWorld) {
    // Declared Increased then an undeclared type as the newest page.
    w.dispatch(conf::pm_trigger_request(
        "counter",
        &["test.counter.Increased", "test.counter.Unwatched"],
        None,
        &[],
    ));
}

#[when(regex = r"^an Increased trigger is dispatched over a prior state of (\d+) events$")]
async fn increased_over_state(w: &mut ProcessManagerWorld, n: u32) {
    w.dispatch(conf::pm_trigger_request(
        "counter",
        &["test.counter.Increased"],
        Some(conf::pm_state_of(n)),
        &[],
    ));
}

#[when("a request with no trigger is dispatched")]
async fn no_trigger(w: &mut ProcessManagerWorld) {
    w.dispatch(conf::pm_missing_trigger());
}

#[when("a trigger with no pages is dispatched")]
async fn empty_trigger(w: &mut ProcessManagerWorld) {
    w.dispatch(conf::pm_empty_trigger());
}

#[when("a rejection of Reserve is dispatched")]
async fn rejection_reserve(w: &mut ProcessManagerWorld) {
    w.dispatch(conf::pm_rejection_request("test.counter.Reserve"));
}

#[then(regex = r#"^the process-manager emits one command to "([^"]*)"$"#)]
async fn emits_one_command(w: &mut ProcessManagerWorld, target: String) {
    let resp = w.response();
    assert_eq!(resp.commands.len(), 1, "exactly one command emitted");
    assert_eq!(resp.commands[0].cover.as_ref().expect("cover").domain, target);
}

#[then(regex = r"^the command carries destination sequence (\d+)$")]
async fn command_carries_sequence(w: &mut ProcessManagerWorld, seq: u32) {
    let cmd = &w.response().commands[0];
    let got = match cmd.pages[0].header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
        Some(pb::page_header::SequenceType::Sequence(s)) => *s,
        _ => panic!("command page carries no explicit sequence"),
    };
    assert_eq!(got, seq);
}

#[then("the process-manager emits no commands")]
async fn emits_no_commands(w: &mut ProcessManagerWorld) {
    assert!(w.response().commands.is_empty(), "no commands emitted");
}

#[then(regex = r"^the process-manager rebuilt (\d+) prior state events$")]
async fn rebuilt_n(w: &mut ProcessManagerWorld, n: u32) {
    assert_eq!(
        w.response().facts.len() as u32,
        n,
        "one fact per rebuilt prior state event"
    );
}

#[then("the process-manager emits one process event")]
async fn emits_one_process_event(w: &mut ProcessManagerWorld) {
    assert_eq!(w.response().process_events.len(), 1, "one process event");
}

#[then("the process-manager escalates")]
async fn escalates(w: &mut ProcessManagerWorld) {
    assert!(w.response().notification.is_some(), "an escalation was raised");
}

#[then(regex = r"^the dispatch fails with ([A-Z_]+)$")]
async fn fails_with(w: &mut ProcessManagerWorld, code: String) {
    let err = w
        .result
        .as_ref()
        .expect("a trigger was dispatched")
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("expected failure {code}, got a success"));
    assert_eq!(err.code, code, "coded-error reason");
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/features/process_manager.feature"
    );
    ProcessManagerWorld::cucumber().run_and_exit(feature).await;
}
