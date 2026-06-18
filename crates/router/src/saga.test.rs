//! SagaDispatch contracts, transliterated from client-go's engine.go
//! SagaDispatch.Dispatch + features/saga.go (spec C-0050..C-0053, C-0042):
//! a stateless translator that walks EVERY source page, emits commands
//! (stamped from coordinator Destinations) and/or injected fact events for
//! declared event types, routes Notification pages to ordered compensation
//! thunks, skips undeclared events/rejections (DelegateToFramework), and
//! fills the source correlation id onto emitted commands fill-only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use prost_types::Any;

use crate::error::{codes, HandlerError};
use crate::pb;
use crate::saga::SagaDispatch;
use crate::test_support::{event_page, notification_page_for};
use crate::{type_url, NOTIFICATION_TYPE_URL};

const FQ_ORDER_CREATED: &str = "test.OrderCreated";
const FQ_STOCK_RESERVED: &str = "test.StockReserved";
const FQ_RESERVE_STOCK: &str = "test.ReserveStock";

// --- fixtures -------------------------------------------------------------

/// A SagaHandleRequest over an optional source book and a destination map.
fn request(source: Option<pb::EventBook>, dest: &[(&str, u32)]) -> pb::SagaHandleRequest {
    pb::SagaHandleRequest {
        source,
        destination_sequences: dest.iter().map(|(d, s)| (d.to_string(), *s)).collect(),
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

/// A fact EventBook tagged with `label` in its cover domain, so emission
/// order and origin are observable in the merged response.
fn fact_event(label: &str) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: label.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn cmd_to<'a>(resp: &'a pb::SagaResponse, domain: &str) -> &'a pb::CommandBook {
    resp.commands
        .iter()
        .find(|c| c.cover.as_ref().map(|cv| cv.domain.as_str()) == Some(domain))
        .unwrap_or_else(|| panic!("no command targets {domain}"))
}

fn cmd_page_seq(page: &pb::CommandPage) -> Option<u32> {
    match page.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
        Some(pb::page_header::SequenceType::Sequence(s)) => Some(*s),
        _ => None,
    }
}

fn event_domains(resp: &pb::SagaResponse) -> Vec<String> {
    resp.events
        .iter()
        .map(|e| e.cover.as_ref().map(|c| c.domain.clone()).unwrap_or_default())
        .collect()
}

// --- emission -------------------------------------------------------------

#[test]
fn declared_event_emits_its_command() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _d| Ok((vec![command_to("inventory")], vec![])));
    let resp = saga
        .dispatch(&request(Some(source_book("order", vec![event_page_of(FQ_ORDER_CREATED)])), &[]))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 1, "one command emitted (C-0050)");
    assert_eq!(cmd_to(&resp, "inventory").cover.as_ref().unwrap().domain, "inventory");
}

#[test]
fn undeclared_event_type_is_skipped() {
    // Only OrderCreated is declared; a StockReserved page emits nothing and
    // is not an error (spec C-0051).
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _d| Ok((vec![command_to("inventory")], vec![])));
    let resp = saga
        .dispatch(&request(Some(source_book("order", vec![event_page_of(FQ_STOCK_RESERVED)])), &[]))
        .expect("dispatch");
    assert!(resp.commands.is_empty(), "undeclared event emits no commands");
}

#[test]
fn every_page_is_a_fresh_trigger() {
    // The saga walks EVERY page (source = triggering events, not state):
    // three OrderCreated pages each emit one command → three commands.
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _d| Ok((vec![command_to("inventory")], vec![])));
    let pages = (0..3).map(|_| event_page_of(FQ_ORDER_CREATED)).collect();
    let resp = saga
        .dispatch(&request(Some(source_book("order", pages)), &[]))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 3, "every page triggers the handler");
}

#[test]
fn event_thunk_can_inject_fact_events() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _d| Ok((vec![], vec![fact_event("fact")])));
    let resp = saga
        .dispatch(&request(Some(source_book("order", vec![event_page_of(FQ_ORDER_CREATED)])), &[]))
        .expect("dispatch");
    assert!(resp.commands.is_empty());
    assert_eq!(event_domains(&resp), vec!["fact".to_string()]);
}

// --- destinations (C-0052, C-0053) ---------------------------------------

#[test]
fn two_target_fanout_stamps_destination_sequences() {
    // Handler emits one stamped command per target; the coordinator supplied
    // inventory=7, fulfillment=3 → each command page carries its sequence.
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory", "fulfillment"])
        .on_event(FQ_ORDER_CREATED, |_e, dests| {
            let mut inv = command_to("inventory");
            dests.stamp_command(&mut inv, "inventory")?;
            let mut ful = command_to("fulfillment");
            dests.stamp_command(&mut ful, "fulfillment")?;
            Ok((vec![inv, ful], vec![]))
        });
    let resp = saga
        .dispatch(&request(
            Some(source_book("order", vec![event_page_of(FQ_ORDER_CREATED)])),
            &[("inventory", 7), ("fulfillment", 3)],
        ))
        .expect("dispatch");
    assert_eq!(cmd_page_seq(&cmd_to(&resp, "inventory").pages[0]), Some(7), "C-0053");
    assert_eq!(cmd_page_seq(&cmd_to(&resp, "fulfillment").pages[0]), Some(3), "C-0053");
}

#[test]
fn handler_observes_destination_sequences() {
    // The handler reads the coordinator-supplied sequences (C-0052).
    let observed: Arc<Mutex<HashMap<String, u32>>> = Arc::new(Mutex::new(HashMap::new()));
    let cap = observed.clone();
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]).on_event(
        FQ_ORDER_CREATED,
        move |_e, dests| {
            for d in dests.domains() {
                if let Some(s) = dests.sequence_for(&d) {
                    cap.lock().unwrap().insert(d, s);
                }
            }
            Ok((vec![], vec![]))
        },
    );
    saga.dispatch(&request(
        Some(source_book("order", vec![event_page_of(FQ_ORDER_CREATED)])),
        &[("inventory", 5)],
    ))
    .expect("dispatch");
    assert_eq!(observed.lock().unwrap().get("inventory"), Some(&5));
}

// --- rejection fan-out (C-0042) ------------------------------------------

#[test]
fn notification_routes_to_ordered_rejection_thunks() {
    // Two compensators for the same command run in REGISTRATION order
    // (C-0042); their fact events merge in that order.
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_rejected(FQ_RESERVE_STOCK, |_n, _r| Ok(vec![fact_event("comp-1")]))
        .on_rejected(FQ_RESERVE_STOCK, |_n, _r| Ok(vec![fact_event("comp-2")]));
    let resp = saga
        .dispatch(&request(
            Some(source_book("order", vec![notification_page_for(FQ_RESERVE_STOCK)])),
            &[],
        ))
        .expect("dispatch");
    assert_eq!(event_domains(&resp), vec!["comp-1".to_string(), "comp-2".to_string()]);
}

#[test]
fn undeclared_rejection_yields_empty_response() {
    // A notification for a command with no registered compensator is the
    // framework's to handle (DelegateToFramework): empty, not an error.
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]);
    let resp = saga
        .dispatch(&request(
            Some(source_book("order", vec![notification_page_for(FQ_RESERVE_STOCK)])),
            &[],
        ))
        .expect("dispatch");
    assert!(resp.events.is_empty());
    assert!(resp.commands.is_empty());
}

// --- correlation (fill-only) ---------------------------------------------

#[test]
fn correlation_fills_only_unset_command_covers() {
    // The source correlation id flows onto emitted commands that did not set
    // their own — a command that stamped its own correlation keeps it.
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory", "fulfillment"])
        .on_event(FQ_ORDER_CREATED, |_e, _d| {
            let inherit = command_to("inventory");
            let mut own = command_to("fulfillment");
            own.cover.as_mut().unwrap().correlation_id = "own".to_string();
            Ok((vec![inherit, own], vec![]))
        });
    let mut src = source_book("order", vec![event_page_of(FQ_ORDER_CREATED)]);
    src.cover.as_mut().unwrap().correlation_id = "corr-1".to_string();
    let resp = saga.dispatch(&request(Some(src), &[])).expect("dispatch");
    assert_eq!(
        cmd_to(&resp, "inventory").cover.as_ref().unwrap().correlation_id,
        "corr-1",
        "unset cover inherits source correlation"
    );
    assert_eq!(
        cmd_to(&resp, "fulfillment").cover.as_ref().unwrap().correlation_id,
        "own",
        "fill-only: a handler-set correlation is preserved"
    );
}

// --- envelope + error guards ---------------------------------------------

#[test]
fn nil_source_is_missing_saga_source() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]);
    let err = saga.dispatch(&request(None, &[])).expect_err("nil source must fail");
    assert_eq!(err.code, codes::MISSING_SAGA_SOURCE);
}

#[test]
fn empty_source_is_empty_saga_source() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]);
    let err = saga
        .dispatch(&request(Some(source_book("order", vec![])), &[]))
        .expect_err("empty source must fail");
    assert_eq!(err.code, codes::EMPTY_SAGA_SOURCE);
}

#[test]
fn corrupt_notification_payload_is_coded() {
    // A page claiming the Notification type but carrying undecodable bytes
    // fails with NOTIFICATION_DECODE_FAILED.
    let bad = event_page(Any {
        type_url: NOTIFICATION_TYPE_URL.to_string(),
        value: vec![0xFF, 0xFF, 0xFF, 0xFF],
    });
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_rejected(FQ_RESERVE_STOCK, |_n, _r| Ok(vec![]));
    let err = saga
        .dispatch(&request(Some(source_book("order", vec![bad])), &[]))
        .expect_err("corrupt notification must fail");
    assert_eq!(err.code, codes::NOTIFICATION_DECODE_FAILED);
}

#[test]
fn handler_error_propagates_as_unhandled() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _d| Err(HandlerError::Other("boom".to_string())));
    let err = saga
        .dispatch(&request(Some(source_book("order", vec![event_page_of(FQ_ORDER_CREATED)])), &[]))
        .expect_err("handler error must fail dispatch");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
}

// --- accessors ------------------------------------------------------------

#[test]
fn accessors_report_name_domains_and_types() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory", "fulfillment"])
        .on_event(FQ_ORDER_CREATED, |_e, _d| Ok((vec![], vec![])));
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
