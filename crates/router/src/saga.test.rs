//! SagaDispatch contracts, transliterated from client-go's engine.go
//! SagaDispatch.Dispatch + features/saga.go (spec C-0050, C-0051, C-0042):
//! a stateless translator that walks EVERY source page, emits commands for
//! declared event types, skips undeclared events (DelegateToFramework), and
//! leaves correlation to the coordinator.

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

// --- correlation (coordinator-owned) -----------------------------------

#[test]
fn router_does_not_fill_command_correlation() {
    // The coordinator stamps correlation; the router passes the handler's
    // commands through untouched even when the source carries one.
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _c| Ok(vec![command_to("inventory")]));
    let mut src = source_book("order", vec![event_page_of(FQ_ORDER_CREATED)]);
    src.cover.as_mut().unwrap().correlation_id = "corr-1".to_string();
    let resp = saga.dispatch(&request(Some(src))).expect("dispatch");
    assert_eq!(
        cmd_to(&resp, "inventory")
            .cover
            .as_ref()
            .unwrap()
            .correlation_id,
        "",
        "router must not fill correlation"
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
