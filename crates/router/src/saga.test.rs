//! SagaDispatch contracts (spec C-0050..C-0053): a stateless translator
//! that walks EVERY source page, emits commands for declared event types
//! (passed through unstamped: the coordinator stamps identity, causation
//! and correlation) and skips undeclared events.

use std::sync::{Arc, Mutex};

use prost_types::Any;

use crate::error::{codes, HandlerError};
use crate::pb;
use crate::saga::SagaDispatch;
use crate::test_support::event_page;
use crate::type_url;

const FQ_ORDER_CREATED: &str = "test.OrderCreated";
const FQ_STOCK_RESERVED: &str = "test.StockReserved";

// --- fixtures -------------------------------------------------------------

/// A SagaHandleRequest over an optional source book.
fn request(source: Option<pb::EventBook>) -> pb::SagaHandleRequest {
    pb::SagaHandleRequest {
        source,
        ..Default::default()
    }
}

/// A source EventBook in `domain` over the given pages.
fn source_book(domain: &str, pages: Vec<pb::EventPage>) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages,
        ..Default::default()
    }
}

/// An event page carrying a bare `Any` of the fully-qualified type (no
/// payload bytes — these sagas react to type, not content).
fn event_page_of(fq: &str) -> pb::EventPage {
    event_page(Any {
        type_url: type_url(fq),
        value: Vec::new(),
    })
}

/// A command book targeting `domain` with one command page.
fn command_to(domain: &str) -> pb::CommandBook {
    pb::CommandBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages: vec![pb::CommandPage {
            payload: Some(pb::command_page::Payload::Command(Any {
                type_url: type_url("test.Command"),
                value: Vec::new(),
            })),
            ..Default::default()
        }],
    }
}

fn cmd_to<'a>(resp: &'a pb::SagaResponse, domain: &str) -> &'a pb::CommandBook {
    resp.commands
        .iter()
        .find(|c| c.cover.as_ref().map(|cv| cv.domain.as_str()) == Some(domain))
        .unwrap_or_else(|| panic!("no command targets {domain}"))
}

fn sequenced_page_of(fq: &str, seq: u32) -> pb::EventPage {
    let mut page = event_page_of(fq);
    page.header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(seq)),
        ..Default::default()
    });
    page
}

// --- emission -------------------------------------------------------------

#[test]
fn declared_event_emits_its_command() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _c| Ok(vec![command_to("inventory")]));
    let resp = saga
        .dispatch(&request(Some(source_book(
            "order",
            vec![event_page_of(FQ_ORDER_CREATED)],
        ))))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 1, "one command emitted (C-0050)");
    assert_eq!(
        cmd_to(&resp, "inventory").cover.as_ref().unwrap().domain,
        "inventory"
    );
}

#[test]
fn undeclared_event_type_is_skipped() {
    // Only OrderCreated is declared; a StockReserved page emits nothing and
    // is not an error (spec C-0051).
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _c| Ok(vec![command_to("inventory")]));
    let resp = saga
        .dispatch(&request(Some(source_book(
            "order",
            vec![event_page_of(FQ_STOCK_RESERVED)],
        ))))
        .expect("dispatch");
    assert!(
        resp.commands.is_empty(),
        "undeclared event emits no commands"
    );
}

#[test]
fn every_page_is_a_fresh_trigger() {
    // The saga walks EVERY page (source = triggering events, not state):
    // three OrderCreated pages each emit one command → three commands.
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _c| Ok(vec![command_to("inventory")]));
    let pages = (0..3).map(|_| event_page_of(FQ_ORDER_CREATED)).collect();
    let resp = saga
        .dispatch(&request(Some(source_book("order", pages))))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 3, "every page triggers the handler");
}

// --- pass-through: the coordinator stamps emitted commands ---------------

#[test]
fn emitted_commands_pass_through_unstamped_in_emission_order() {
    // A handler-set header and correlation survive untouched; nothing is
    // filled in from the source.
    let saga = SagaDispatch::new("OrderSplit", "order", ["inventory", "fulfillment"]).on_event(
        FQ_ORDER_CREATED,
        |_e, _c| {
            let mut own = command_to("inventory");
            own.pages[0].header = Some(pb::PageHeader {
                sequence_type: Some(pb::page_header::SequenceType::Sequence(9)),
                ..Default::default()
            });
            Ok(vec![own, command_to("fulfillment")])
        },
    );
    let mut src = source_book(
        "order",
        vec![
            sequenced_page_of(FQ_ORDER_CREATED, 1),
            sequenced_page_of(FQ_ORDER_CREATED, 2),
        ],
    );
    src.cover.as_mut().unwrap().correlation_id = "corr-1".to_string();
    let resp = saga.dispatch(&request(Some(src))).expect("dispatch");

    let mut expected_first = command_to("inventory");
    expected_first.pages[0].header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(9)),
        ..Default::default()
    });
    assert_eq!(
        resp.commands,
        vec![
            expected_first.clone(),
            command_to("fulfillment"),
            expected_first,
            command_to("fulfillment"),
        ],
        "commands are returned exactly as emitted, page by page"
    );
}

// --- envelope + error guards ---------------------------------------------

#[test]
fn nil_source_is_missing_saga_source() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]);
    let err = saga
        .dispatch(&request(None))
        .expect_err("nil source must fail");
    assert_eq!(err.code, codes::MISSING_SAGA_SOURCE);
}

#[test]
fn empty_source_is_empty_saga_source() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]);
    let err = saga
        .dispatch(&request(Some(source_book("order", vec![]))))
        .expect_err("empty source must fail");
    assert_eq!(err.code, codes::EMPTY_SAGA_SOURCE);
}

#[test]
fn handler_error_propagates_as_unhandled() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _c| {
            Err(HandlerError::Other("boom".to_string()))
        });
    let err = saga
        .dispatch(&request(Some(source_book(
            "order",
            vec![event_page_of(FQ_ORDER_CREATED)],
        ))))
        .expect_err("handler error must fail dispatch");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
}

// --- accessors ------------------------------------------------------------

#[test]
fn accessors_report_name_domains_and_types() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory", "fulfillment"])
        .on_event(FQ_ORDER_CREATED, |_e, _c| Ok(vec![]));
    assert_eq!(saga.name(), "OrderFulfillment");
    assert_eq!(saga.input_domain(), "order");
    assert_eq!(
        saga.target_domains(),
        &["inventory".to_string(), "fulfillment".to_string()]
    );
    assert_eq!(saga.event_types(), vec![FQ_ORDER_CREATED.to_string()]);
    assert_eq!(
        saga.subscriptions().get("order"),
        Some(&vec![FQ_ORDER_CREATED.to_string()])
    );
}

// --- handler context: the triggering page's cover and sequence (X-037) ----

#[test]
fn a_context_handler_sees_each_triggering_page() {
    let seen: Arc<Mutex<Vec<(String, u32)>>> = Arc::default();
    let sink = seen.clone();
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]).on_event_with_context(
        FQ_ORDER_CREATED,
        move |_e, source| {
            let domain = source.cover.map(|c| c.domain.clone()).unwrap_or_default();
            sink.lock().unwrap().push((domain, source.sequence));
            Ok(vec![command_to("inventory")])
        },
    );
    let resp = saga
        .dispatch(&request(Some(source_book(
            "order",
            vec![
                sequenced_page_of(FQ_ORDER_CREATED, 4),
                sequenced_page_of(FQ_STOCK_RESERVED, 5),
                sequenced_page_of(FQ_ORDER_CREATED, 6),
            ],
        ))))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 2, "one command per declared page");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![("order".to_string(), 4), ("order".to_string(), 6)]
    );
}

#[test]
fn a_context_handler_sees_sequence_zero_for_an_unsequenced_page() {
    let seen: Arc<Mutex<Vec<u32>>> = Arc::default();
    let sink = seen.clone();
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]).on_event_with_context(
        FQ_ORDER_CREATED,
        move |_e, source| {
            sink.lock().unwrap().push(source.sequence);
            Ok(vec![])
        },
    );
    saga.dispatch(&request(Some(source_book(
        "order",
        vec![event_page_of(FQ_ORDER_CREATED)],
    ))))
    .expect("dispatch");
    assert_eq!(*seen.lock().unwrap(), vec![0]);
}
