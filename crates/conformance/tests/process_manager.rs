//! Cucumber harness for the process-manager behavior suite: drives the shared
//! `process_manager.feature` against the Rust core's ProcessManagerDispatch
//! natively. The same feature runs against the bindings and the generated
//! clients in later units — only this step-definition layer differs.

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router::process_manager::{
    merge_response, select_process_managers, ProcessManagerRoute,
};
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct ProcessManagerWorld {
    /// True when the audit PM is co-resident with the order PM.
    co_resident: bool,
    /// The (code, message) of each rejection the order PM compensated.
    seen: conf::RejectionSink,
    /// Outcome of the dispatched trigger.
    result: Option<Result<pb::ProcessManagerHandleResponse, CodedError>>,
}

impl ProcessManagerWorld {
    fn dispatch(&mut self, req: pb::ProcessManagerHandleRequest) {
        if !self.co_resident {
            self.result = Some(conf::order_pm(self.seen.clone()).dispatch(&req));
            return;
        }
        // Co-resident: the router's selection + merge over both PMs, each
        // dispatching over its own state type.
        let order = conf::order_pm(self.seen.clone());
        let audit = conf::audit_pm();
        let routes: [&dyn ProcessManagerRoute; 2] = [&order, &audit];
        let mut merged = pb::ProcessManagerHandleResponse::default();
        for index in select_process_managers(&routes, &req) {
            let resp = match index {
                0 => order.dispatch(&req),
                _ => audit.dispatch(&req),
            };
            match resp {
                Ok(resp) => merge_response(&mut merged, resp),
                Err(err) => {
                    self.result = Some(Err(err));
                    return;
                }
            }
        }
        self.result = Some(Ok(merged));
    }

    fn facts_marked(&self, audit: bool) -> usize {
        self.response()
            .facts
            .iter()
            .filter(|f| {
                let domain = f.cover.as_ref().map_or("", |c| c.domain.as_str());
                (domain == conf::AUDIT_MARK) == audit
            })
            .count()
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

#[when(regex = r#"^an Increased trigger in domain "([^"]*)" at sequence (\d+) is dispatched$"#)]
async fn increased_at(w: &mut ProcessManagerWorld, domain: String, seq: u32) {
    w.dispatch(conf::pm_trigger_request(
        &domain,
        &["test.counter.Increased"],
        None,
        Some(seq),
    ));
}

#[when("a Compensate for Reserve is dispatched to the order process-manager")]
async fn compensate_for_reserve(w: &mut ProcessManagerWorld) {
    w.dispatch(conf::pm_compensate_request("Reserve"));
}

#[when(regex = r#"^an Increased trigger in domain "([^"]*)" is dispatched$"#)]
async fn increased_in_domain(w: &mut ProcessManagerWorld, domain: String) {
    w.dispatch(conf::pm_trigger_request(
        &domain,
        &["test.counter.Increased"],
        None,
        None,
    ));
}

#[when("a trigger whose newest page is an undeclared event is dispatched")]
async fn newest_undeclared(w: &mut ProcessManagerWorld) {
    // Declared Increased then an undeclared type as the newest page.
    w.dispatch(conf::pm_trigger_request(
        "counter",
        &["test.counter.Increased", "test.counter.Unwatched"],
        None,
        None,
    ));
}

#[when(regex = r"^an Increased trigger is dispatched over a prior state of (\d+) events$")]
async fn increased_over_state(w: &mut ProcessManagerWorld, n: u32) {
    w.dispatch(conf::pm_trigger_request(
        "counter",
        &["test.counter.Increased"],
        Some(conf::pm_state_of(n)),
        None,
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
    assert_eq!(
        resp.commands[0].cover.as_ref().expect("cover").domain,
        target
    );
}

#[then(regex = r"^the command is deferred from source sequence (\d+) at command index (\d+)$")]
async fn command_is_deferred(w: &mut ProcessManagerWorld, seq: u32, index: u32) {
    let cmd = &w.response().commands[0];
    for page in &cmd.pages {
        let Some(pb::page_header::SequenceType::AngzarrDeferred(d)) =
            page.header.as_ref().and_then(|h| h.sequence_type.as_ref())
        else {
            panic!("command page is not deferred");
        };
        assert_eq!(d.source_seq, seq, "source_seq is the trigger's");
        assert_eq!(
            d.command_index, index,
            "command_index is the emission position"
        );
        assert_eq!(
            d.source.as_ref().map(|c| c.domain.as_str()),
            Some("counter"),
            "the source cover is the trigger book's"
        );
    }
}

#[then("the command leaves its source component to the coordinator")]
async fn no_source_component(w: &mut ProcessManagerWorld) {
    for page in &w.response().commands[0].pages {
        let Some(pb::page_header::SequenceType::AngzarrDeferred(d)) =
            page.header.as_ref().and_then(|h| h.sequence_type.as_ref())
        else {
            panic!("command page is not deferred");
        };
        assert_eq!(
            d.source_component, "",
            "the coordinator stamps the component"
        );
    }
}

#[when(
    regex = r#"^a rejection of Reserve with code "([^"]*)" and message "([^"]*)" is dispatched$"#
)]
async fn rejection_with_code(w: &mut ProcessManagerWorld, code: String, message: String) {
    w.dispatch(conf::pm_rejection_with(
        "test.counter.Reserve",
        &code,
        &message,
    ));
}

#[then(regex = r#"^the process-manager compensator saw code "([^"]*)" and message "([^"]*)"$"#)]
async fn compensator_saw_code(w: &mut ProcessManagerWorld, code: String, message: String) {
    assert_eq!(*w.seen.lock().unwrap(), vec![(code, message)]);
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

#[then(regex = r#"^the process event is addressed to "([^"]*)"$"#)]
async fn process_event_addressed_to(w: &mut ProcessManagerWorld, domain: String) {
    let books = &w.response().process_events;
    assert_eq!(books.len(), 1, "one process event");
    assert_eq!(
        books[0].cover.as_ref().map(|c| c.domain.as_str()),
        Some(domain.as_str())
    );
}

#[then("the process-manager escalates")]
async fn escalates(w: &mut ProcessManagerWorld) {
    assert!(
        w.response().notification.is_some(),
        "an escalation was raised"
    );
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

#[given("co-resident order and audit process-managers")]
async fn co_resident_pms(w: &mut ProcessManagerWorld) {
    w.co_resident = true;
}

#[when(
    regex = r#"^an Increased trigger is dispatched over a prior "([^"]*)" state of (\d+) events$"#
)]
async fn increased_over_owned_state(w: &mut ProcessManagerWorld, owner: String, n: u32) {
    w.dispatch(conf::pm_trigger_request(
        "counter",
        &["test.counter.Increased"],
        Some(conf::pm_state_in(&owner, n)),
        None,
    ));
}

#[when(regex = r#"^a rejection of Reserve issued by "([^"]*)" is dispatched$"#)]
async fn rejection_issued_by(w: &mut ProcessManagerWorld, issuer: String) {
    w.dispatch(conf::pm_issued_rejection_request(
        "test.counter.Reserve",
        &issuer,
    ));
}

#[then(regex = r"^the order process-manager rebuilt (\d+) prior state events$")]
async fn order_rebuilt(w: &mut ProcessManagerWorld, n: usize) {
    assert_eq!(
        w.facts_marked(false),
        n,
        "order PM facts = its rebuilt events"
    );
}

#[then(regex = r"^the audit process-manager rebuilt (\d+) prior state events$")]
async fn audit_rebuilt(w: &mut ProcessManagerWorld, n: usize) {
    assert_eq!(
        w.facts_marked(true),
        n,
        "audit PM facts = its rebuilt events"
    );
}

#[then("the order process-manager did not react")]
async fn order_did_not_react(w: &mut ProcessManagerWorld) {
    assert!(
        w.response().commands.is_empty(),
        "the order PM emitted no command"
    );
    assert_eq!(w.facts_marked(false), 0, "the order PM emitted no fact");
}

#[then("only the audit process-manager compensates")]
async fn only_audit_compensates(w: &mut ProcessManagerWorld) {
    let resp = w.response();
    assert_eq!(resp.process_events.len(), 1, "exactly one compensation");
    assert_eq!(
        resp.process_events[0]
            .cover
            .as_ref()
            .map(|c| c.domain.as_str()),
        Some(conf::AUDIT_MARK),
        "the audit PM compensated"
    );
    assert!(resp.notification.is_none(), "the order PM did not escalate");
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/features/process_manager.feature"
    );
    ProcessManagerWorld::cucumber().run_and_exit(feature).await;
}
