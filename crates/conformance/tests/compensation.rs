//! Cucumber harness for the compensation-routing suite: drives the shared
//! `compensation.feature` against hand-built aggregates on the Rust core
//! natively, dispatched through a Router so several aggregates can share a
//! domain. The bindings run the same feature through their own hand-written
//! aggregate APIs and routers.

use angzarr_router::aggregate::AggregateDispatch;
use angzarr_router::error::{CodedError, GrpcCode};
use angzarr_router::pb;
use angzarr_router::router::RouterBuilder;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

#[derive(Default, World)]
struct CompensationWorld {
    aggregates: Vec<AggregateDispatch<()>>,
    result: Option<Result<pb::BusinessResponse, CodedError>>,
}

impl std::fmt::Debug for CompensationWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompensationWorld")
            .field("result", &self.result)
            .finish()
    }
}

impl CompensationWorld {
    fn dispatch(&mut self, cmd: pb::ContextualCommand) {
        let aggregates = std::mem::take(&mut self.aggregates);
        assert!(!aggregates.is_empty(), "an aggregate was built");
        let router = aggregates
            .into_iter()
            .fold(RouterBuilder::new(), RouterBuilder::aggregate)
            .build()
            .expect("the router builds");
        self.result = Some(router.dispatch_command(&cmd));
    }

    fn pages(&self) -> Vec<pb::EventPage> {
        let resp = self
            .result
            .as_ref()
            .expect("a notification was dispatched")
            .as_ref()
            .expect("expected a successful response");
        match &resp.result {
            Some(pb::business_response::Result::Events(book)) => book.pages.clone(),
            None => Vec::new(),
            other => panic!("expected events, got {other:?}"),
        }
    }
}

#[given(regex = r"^a payment aggregate compensating Reserve from any domain with (\w+)$")]
async fn payment_unqualified(w: &mut CompensationWorld, event: String) {
    w.aggregates
        .push(conf::payment_aggregate(&[("test.counter.Reserve", &event)]));
}

#[given(regex = r"^a second payment aggregate compensating Reserve from any domain with (\w+)$")]
async fn second_payment(w: &mut CompensationWorld, event: String) {
    w.aggregates
        .push(conf::payment_aggregate(&[("test.counter.Reserve", &event)]));
}

#[given(
    regex = r#"^a payment aggregate compensating Reserve from "([^"]*)" with (\w+) and from "([^"]*)" with (\w+)$"#
)]
async fn payment_qualified(
    w: &mut CompensationWorld,
    first_domain: String,
    first_event: String,
    second_domain: String,
    second_event: String,
) {
    let first = format!("{first_domain}:test.counter.Reserve");
    let second = format!("{second_domain}:test.counter.Reserve");
    w.aggregates.push(conf::payment_aggregate(&[
        (&first, &first_event),
        (&second, &second_event),
    ]));
}

#[given(
    "an inventory aggregate undoing AdjustStock with StockAdjustmentReverted and Reserve with StockReleased"
)]
async fn inventory(w: &mut CompensationWorld) {
    w.aggregates.push(conf::inventory_aggregate());
}

#[when(
    regex = r#"^a rejection of (\w+) sent to "([^"]*)" is dispatched to the payment aggregate$"#
)]
async fn rejection_sent_to(w: &mut CompensationWorld, command: String, domain: String) {
    w.dispatch(conf::rejection_sent_to(&command, &domain, None));
}

#[when(
    regex = r#"^a rejection of (\w+) sent to "([^"]*)" is dispatched to the payment aggregate over history ending at sequence (\d+)$"#
)]
async fn rejection_over_history(
    w: &mut CompensationWorld,
    command: String,
    domain: String,
    last: u32,
) {
    w.dispatch(conf::rejection_sent_to(&command, &domain, Some(last + 1)));
}

#[when(regex = r"^a Compensate for (\w+) is dispatched to the inventory aggregate$")]
async fn compensate(w: &mut CompensationWorld, command: String) {
    w.dispatch(conf::compensate_for(&command));
}

#[then(regex = r"^the aggregate emits one (\w+) event$")]
async fn emits_one(w: &mut CompensationWorld, event: String) {
    let pages = w.pages();
    assert_eq!(pages.len(), 1, "exactly one event");
    let Some(pb::event_page::Payload::Event(any)) = pages[0].payload.as_ref() else {
        panic!("the page carries no event");
    };
    assert_eq!(
        angzarr_router::type_name_from_url(&any.type_url),
        format!("test.counter.{event}")
    );
}

#[then(regex = r"^the aggregates emit (\w+) at sequence (\d+) then (\w+) at sequence (\d+)$")]
async fn emit_in_order(
    w: &mut CompensationWorld,
    first: String,
    first_seq: u32,
    second: String,
    second_seq: u32,
) {
    let got: Vec<(String, u32)> = w
        .pages()
        .iter()
        .map(|page| {
            let Some(pb::event_page::Payload::Event(any)) = page.payload.as_ref() else {
                panic!("the page carries no event");
            };
            let name = angzarr_router::type_name_from_url(&any.type_url).to_string();
            (name, explicit_sequence(page))
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (format!("test.counter.{first}"), first_seq),
            (format!("test.counter.{second}"), second_seq),
        ]
    );
}

fn explicit_sequence(page: &pb::EventPage) -> u32 {
    match page.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
        Some(pb::page_header::SequenceType::Sequence(s)) => *s,
        other => panic!("no explicit sequence: {other:?}"),
    }
}

#[then("the aggregate emits nothing")]
async fn emits_nothing(w: &mut CompensationWorld) {
    assert!(w.pages().is_empty(), "no events");
}

#[then(regex = r"^the emitted event takes sequence (\d+)$")]
async fn takes_sequence(w: &mut CompensationWorld, seq: u32) {
    assert_eq!(explicit_sequence(&w.pages()[0]), seq);
}

#[then(regex = r"^the dispatch fails with ([A-Z_]+) as UNIMPLEMENTED$")]
async fn fails_unimplemented(w: &mut CompensationWorld, code: String) {
    let err = w
        .result
        .as_ref()
        .expect("a notification was dispatched")
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("expected failure {code}, got a success"));
    assert_eq!(err.code, code);
    assert_eq!(err.grpc, GrpcCode::Unimplemented);
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/features/compensation.feature"
    );
    CompensationWorld::cucumber().run_and_exit(feature).await;
}
