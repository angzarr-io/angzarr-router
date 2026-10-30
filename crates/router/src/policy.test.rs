//! PolicyDispatch contracts (spec C-0050..C-0053): a stateless translator
//! that walks EVERY source page, emits commands for declared event types
//! (passed through unstamped: the coordinator stamps identity, causation
//! and correlation) and skips undeclared events.

use std::sync::{Arc, Mutex};

use prost_types::Any;

use crate::error::{codes, extras, GrpcCode, HandlerError};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::test_support::{command_page, commands_of, event_page, uuid_of};
use crate::type_url;

const FQ_ORDER_CREATED: &str = "test.OrderCreated";
const FQ_STOCK_RESERVED: &str = "test.StockReserved";

// --- fixtures -------------------------------------------------------------

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
/// payload bytes — these policies react to type, not content).
fn event_page_of(fq: &str) -> pb::EventPage {
    event_page(Any {
        type_url: type_url(fq),
        value: Vec::new(),
    })
}

/// A command book targeting `domain` (root 1: a command to an aggregate
/// names its root, decision 14.36) with one command page.
fn command_to(domain: &str) -> pb::CommandBook {
    pb::CommandBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            root: Some(uuid_of(1)),
            ..Default::default()
        }),
        page: Some(command_page(Any {
            type_url: type_url("test.Command"),
            value: Vec::new(),
        })),
        ..Default::default()
    }
}

fn cmd_to(resp: &pb::PolicyResponse, domain: &str) -> pb::CommandBook {
    commands_of(resp)
        .into_iter()
        .find(|c| c.cover.as_ref().map(|cv| cv.domain.as_str()) == Some(domain))
        .unwrap_or_else(|| panic!("no command targets {domain}"))
}

fn sequenced_page_of(fq: &str, seq: u64) -> pb::EventPage {
    let mut page = event_page_of(fq);
    page.sequence = seq;
    page
}

// --- emission -------------------------------------------------------------

#[test]
fn declared_event_emits_its_command() {
    let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory")
        .on_event(FQ_ORDER_CREATED, |_e, _c| Ok(vec![command_to("inventory")]));
    let resp = policy
        .dispatch(&source_book("order", vec![event_page_of(FQ_ORDER_CREATED)]))
        .expect("dispatch");
    assert_eq!(commands_of(&resp).len(), 1, "one command emitted (C-0050)");
    assert_eq!(
        cmd_to(&resp, "inventory").cover.as_ref().unwrap().domain,
        "inventory"
    );
}

#[test]
fn undeclared_event_type_is_skipped() {
    // Only OrderCreated is declared; a StockReserved page emits nothing and
    // is not an error (spec C-0051).
    let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory")
        .on_event(FQ_ORDER_CREATED, |_e, _c| Ok(vec![command_to("inventory")]));
    let resp = policy
        .dispatch(&source_book(
            "order",
            vec![event_page_of(FQ_STOCK_RESERVED)],
        ))
        .expect("dispatch");
    assert!(
        commands_of(&resp).is_empty(),
        "undeclared event emits no commands"
    );
}

#[test]
fn every_page_is_a_fresh_trigger() {
    // The policy walks EVERY page (source = triggering events, not state):
    // three OrderCreated pages each emit one command → three commands.
    let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory")
        .on_event(FQ_ORDER_CREATED, |_e, _c| Ok(vec![command_to("inventory")]));
    let pages = (0..3).map(|_| event_page_of(FQ_ORDER_CREATED)).collect();
    let resp = policy
        .dispatch(&source_book("order", pages))
        .expect("dispatch");
    assert_eq!(
        commands_of(&resp).len(),
        3,
        "every page triggers the handler"
    );
}

// --- pass-through: the coordinator stamps emitted commands ---------------

#[test]
fn emitted_commands_pass_through_unstamped_in_emission_order() {
    // A handler-set correlation equal to its trigger's survives untouched
    // (another is refused, decision 14.36; addressing.test.rs); nothing is
    // filled in from the source.
    let policy = PolicyDispatch::new("OrderSplit", "order", "inventory").on_event(
        FQ_ORDER_CREATED,
        |_e, _c| {
            let mut own = command_to("inventory");
            own.page.as_mut().unwrap().correlation_id = Some(uuid_of(1));
            Ok(vec![own, command_to("inventory")])
        },
    );
    let mut src = source_book(
        "order",
        vec![
            sequenced_page_of(FQ_ORDER_CREATED, 1),
            sequenced_page_of(FQ_ORDER_CREATED, 2),
        ],
    );
    for page in &mut src.pages {
        page.meta = Some(pb::EventMeta {
            correlation_id: Some(uuid_of(1)),
            ..Default::default()
        });
    }
    let resp = policy.dispatch(&src).expect("dispatch");

    let mut expected_first = command_to("inventory");
    expected_first.page.as_mut().unwrap().correlation_id = Some(uuid_of(1));
    assert_eq!(
        commands_of(&resp),
        vec![
            expected_first.clone(),
            command_to("inventory"),
            expected_first,
            command_to("inventory"),
        ],
        "commands are returned exactly as emitted, page by page"
    );
}

// --- envelope + error guards ---------------------------------------------

#[test]
fn empty_source_is_empty_policy_source() {
    let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory");
    let err = policy
        .dispatch(&source_book("order", vec![]))
        .expect_err("empty source must fail");
    assert_eq!(err.code, codes::EMPTY_POLICY_SOURCE);
    crate::test_support::assert_bug(&err);
}

#[test]
fn handler_error_propagates_as_unhandled() {
    let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory")
        .on_event(FQ_ORDER_CREATED, |_e, _c| {
            Err(HandlerError::Other("boom".to_string()))
        });
    let err = policy
        .dispatch(&source_book("order", vec![event_page_of(FQ_ORDER_CREATED)]))
        .expect_err("handler error must fail dispatch");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
}

// --- one destination (policy.proto PolicyResponse; decision 14.24, C-0583) -----

// A policy translates between exactly two domains: a command for any domain
// but its one destination is a policy bug, refused before it answers.
#[test]
fn a_command_for_a_domain_other_than_the_destination_is_refused_with_no_commands() {
    for stray in ["shipping", ""] {
        let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory")
            .on_event(FQ_ORDER_CREATED, move |_e, _c| {
                Ok(vec![command_to("inventory"), command_to(stray)])
            });
        let err = policy
            .dispatch(&source_book("order", vec![event_page_of(FQ_ORDER_CREATED)]))
            .expect_err("a command outside the destination");
        assert_eq!(err.code, codes::POLICY_COMMAND_OUTSIDE_DESTINATION);
        assert_eq!(err.grpc, GrpcCode::Internal);
        let extra = |key: &str| err.extras.get(key).map(String::as_str);
        assert_eq!(extra(extras::EXPECTED_DOMAIN), Some("inventory"));
        assert_eq!(extra(extras::ACTUAL_DOMAIN), Some(stray));
    }
}

#[test]
fn a_command_with_no_cover_is_outside_the_destination() {
    let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory")
        .on_event(FQ_ORDER_CREATED, |_e, _c| {
            Ok(vec![pb::CommandBook::default()])
        });
    let err = policy
        .dispatch(&source_book("order", vec![event_page_of(FQ_ORDER_CREATED)]))
        .expect_err("no cover names no destination");
    assert_eq!(err.code, codes::POLICY_COMMAND_OUTSIDE_DESTINATION);
}

// --- accessors ------------------------------------------------------------

#[test]
fn accessors_report_name_domains_and_types() {
    let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory")
        .on_event(FQ_ORDER_CREATED, |_e, _c| Ok(vec![]));
    assert_eq!(policy.name(), "OrderFulfillment");
    assert_eq!(policy.input_domain(), "order");
    assert_eq!(policy.output_domain(), "inventory");
    assert_eq!(policy.event_types(), vec![FQ_ORDER_CREATED.to_string()]);
    assert_eq!(
        policy.subscriptions().get("order"),
        Some(&vec![FQ_ORDER_CREATED.to_string()])
    );
}

// --- handler context: the triggering page's cover and sequence (X-037) ----

#[test]
fn a_context_handler_sees_each_triggering_page() {
    let seen: Arc<Mutex<Vec<(String, u64)>>> = Arc::default();
    let sink = seen.clone();
    let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory")
        .on_event_with_context(FQ_ORDER_CREATED, move |_e, source| {
            let domain = source.cover.map(|c| c.domain.clone()).unwrap_or_default();
            sink.lock().unwrap().push((domain, source.sequence));
            Ok(vec![command_to("inventory")])
        });
    let resp = policy
        .dispatch(&source_book(
            "order",
            vec![
                sequenced_page_of(FQ_ORDER_CREATED, 4),
                sequenced_page_of(FQ_STOCK_RESERVED, 5),
                sequenced_page_of(FQ_ORDER_CREATED, 6),
            ],
        ))
        .expect("dispatch");
    assert_eq!(commands_of(&resp).len(), 2, "one command per declared page");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![("order".to_string(), 4), ("order".to_string(), 6)]
    );
}

#[test]
fn a_context_handler_sees_sequence_zero_for_an_unsequenced_page() {
    let seen: Arc<Mutex<Vec<u64>>> = Arc::default();
    let sink = seen.clone();
    let policy = PolicyDispatch::new("OrderFulfillment", "order", "inventory")
        .on_event_with_context(FQ_ORDER_CREATED, move |_e, source| {
            sink.lock().unwrap().push(source.sequence);
            Ok(vec![])
        });
    policy
        .dispatch(&source_book("order", vec![event_page_of(FQ_ORDER_CREATED)]))
        .expect("dispatch");
    assert_eq!(*seen.lock().unwrap(), vec![0]);
}
