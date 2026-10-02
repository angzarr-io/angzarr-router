//! SagaDispatch contracts (spec C-0050..C-0053, C-0177..C-0179): a
//! stateless translator that walks EVERY source page, emits deferred
//! commands (angzarr_deferred provenance from the triggering page, never an
//! explicit sequence) and/or injected fact events for declared event types,
//! skips undeclared events and Notification pages (sagas receive no
//! rejections), and fills the source correlation id onto emitted commands
//! fill-only.

use std::sync::{Arc, Mutex};

use prost_types::Any;

use crate::error::{codes, HandlerError};
use crate::pb;
use crate::saga::SagaDispatch;
use crate::test_support::{event_page, notification_page_for};
use crate::type_url;

const FQ_ORDER_CREATED: &str = "test.OrderCreated";
const FQ_STOCK_RESERVED: &str = "test.StockReserved";
const FQ_RESERVE_STOCK: &str = "test.ReserveStock";

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

fn cmd_page_deferred(page: &pb::CommandPage) -> Option<&pb::AngzarrDeferredSequence> {
    match page.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
        Some(pb::page_header::SequenceType::AngzarrDeferred(d)) => Some(d),
        _ => None,
    }
}

fn sequenced_page_of(fq: &str, seq: u32) -> pb::EventPage {
    let mut page = event_page_of(fq);
    page.header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(seq)),
        ..Default::default()
    });
    page
}

fn event_domains(resp: &pb::SagaResponse) -> Vec<String> {
    resp.events
        .iter()
        .map(|e| {
            e.cover
                .as_ref()
                .map(|c| c.domain.clone())
                .unwrap_or_default()
        })
        .collect()
}

// --- emission -------------------------------------------------------------

#[test]
fn declared_event_emits_its_command() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _d, _c| {
            Ok((vec![command_to("inventory")], vec![]))
        });
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
        .on_event(FQ_ORDER_CREATED, |_e, _d, _c| {
            Ok((vec![command_to("inventory")], vec![]))
        });
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
        .on_event(FQ_ORDER_CREATED, |_e, _d, _c| {
            Ok((vec![command_to("inventory")], vec![]))
        });
    let pages = (0..3).map(|_| event_page_of(FQ_ORDER_CREATED)).collect();
    let resp = saga
        .dispatch(&request(Some(source_book("order", pages))))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 3, "every page triggers the handler");
}

#[test]
fn event_thunk_can_inject_fact_events() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _d, _c| {
            Ok((vec![], vec![fact_event("fact")]))
        });
    let resp = saga
        .dispatch(&request(Some(source_book(
            "order",
            vec![event_page_of(FQ_ORDER_CREATED)],
        ))))
        .expect("dispatch");
    assert!(resp.commands.is_empty());
    assert_eq!(event_domains(&resp), vec!["fact".to_string()]);
}

// --- deferred emission (C-0053, C-0177..C-0179) --------------------------

#[test]
fn emitted_commands_are_deferred_from_their_triggering_page() {
    let saga = SagaDispatch::new("OrderSplit", "order", ["inventory", "fulfillment"]).on_event(
        FQ_ORDER_CREATED,
        |_e, _d, _c| {
            Ok((
                vec![command_to("inventory"), command_to("fulfillment")],
                vec![],
            ))
        },
    );
    let mut src = source_book(
        "order",
        vec![
            sequenced_page_of(FQ_STOCK_RESERVED, 3),
            sequenced_page_of(FQ_ORDER_CREATED, 4),
        ],
    );
    src.cover.as_mut().unwrap().root = Some(pb::Uuid { value: vec![7] });
    let resp = saga
        .dispatch(&request(Some(src.clone())))
        .expect("dispatch");
    for (index, domain) in ["inventory", "fulfillment"].iter().enumerate() {
        let d = cmd_page_deferred(&cmd_to(&resp, domain).pages[0]).expect("deferred header");
        assert_eq!(
            d.source, src.cover,
            "source cover is the triggering book's (C-0177)"
        );
        assert_eq!(
            d.source_seq, 4,
            "source_seq is the triggering page's (C-0177)"
        );
        assert_eq!(
            d.command_index, index as u32,
            "indexed in emission order (C-0179)"
        );
    }
}

#[test]
fn a_handler_set_explicit_sequence_never_survives() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]).on_event(
        FQ_ORDER_CREATED,
        |_e, _d, _c| {
            let mut cmd = command_to("inventory");
            cmd.pages[0].header = Some(pb::PageHeader {
                sequence_type: Some(pb::page_header::SequenceType::Sequence(9)),
                ..Default::default()
            });
            Ok((vec![cmd], vec![]))
        },
    );
    let resp = saga
        .dispatch(&request(Some(source_book(
            "order",
            vec![event_page_of(FQ_ORDER_CREATED)],
        ))))
        .expect("dispatch");
    assert!(
        cmd_page_deferred(&cmd_to(&resp, "inventory").pages[0]).is_some(),
        "C-0178: no explicit sequence"
    );
}

#[test]
fn each_triggering_page_indexes_its_own_commands_from_zero() {
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _d, _c| {
            Ok((vec![command_to("inventory")], vec![]))
        });
    let pages = vec![
        sequenced_page_of(FQ_ORDER_CREATED, 1),
        sequenced_page_of(FQ_ORDER_CREATED, 2),
    ];
    let resp = saga
        .dispatch(&request(Some(source_book("order", pages))))
        .expect("dispatch");
    let stamps: Vec<_> = resp
        .commands
        .iter()
        .map(|c| {
            let d = cmd_page_deferred(&c.pages[0]).unwrap();
            (d.source_seq, d.command_index)
        })
        .collect();
    assert_eq!(stamps, vec![(1, 0), (2, 0)]);
}

#[test]
fn handler_observes_the_declared_output_domains() {
    let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = observed.clone();
    let saga = SagaDispatch::new("OrderSplit", "order", ["inventory", "fulfillment"]).on_event(
        FQ_ORDER_CREATED,
        move |_e, dests, _c| {
            *cap.lock().unwrap() = dests.domains().to_vec();
            Ok((vec![], vec![]))
        },
    );
    saga.dispatch(&request(Some(source_book(
        "order",
        vec![event_page_of(FQ_ORDER_CREATED)],
    ))))
    .expect("dispatch");
    assert_eq!(
        *observed.lock().unwrap(),
        vec!["inventory".to_string(), "fulfillment".to_string()]
    );
}

#[test]
fn a_notification_page_is_skipped() {
    // Sagas receive no rejections: a Notification page is not a declared
    // event and emits nothing, even alongside a declared one.
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"])
        .on_event(FQ_ORDER_CREATED, |_e, _d, _c| {
            Ok((vec![command_to("inventory")], vec![]))
        });
    let resp = saga
        .dispatch(&request(Some(source_book(
            "order",
            vec![
                notification_page_for(FQ_RESERVE_STOCK),
                event_page_of(FQ_ORDER_CREATED),
            ],
        ))))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 1);
    assert!(resp.events.is_empty());
}

// --- correlation (fill-only) ---------------------------------------------

#[test]
fn correlation_fills_only_unset_command_covers() {
    // The source correlation id flows onto emitted commands that did not set
    // their own — a command that stamped its own correlation keeps it.
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory", "fulfillment"])
        .on_event(FQ_ORDER_CREATED, |_e, _d, _c| {
            let inherit = command_to("inventory");
            let mut own = command_to("fulfillment");
            own.cover.as_mut().unwrap().correlation_id = "own".to_string();
            Ok((vec![inherit, own], vec![]))
        });
    let mut src = source_book("order", vec![event_page_of(FQ_ORDER_CREATED)]);
    src.cover.as_mut().unwrap().correlation_id = "corr-1".to_string();
    let resp = saga.dispatch(&request(Some(src))).expect("dispatch");
    assert_eq!(
        cmd_to(&resp, "inventory")
            .cover
            .as_ref()
            .unwrap()
            .correlation_id,
        "corr-1",
        "unset cover inherits source correlation"
    );
    assert_eq!(
        cmd_to(&resp, "fulfillment")
            .cover
            .as_ref()
            .unwrap()
            .correlation_id,
        "own",
        "fill-only: a handler-set correlation is preserved"
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
        .on_event(FQ_ORDER_CREATED, |_e, _d, _c| {
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
        .on_event(FQ_ORDER_CREATED, |_e, _d, _c| Ok((vec![], vec![])));
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
        move |_e, _d, source| {
            let domain = source.cover.map(|c| c.domain.clone()).unwrap_or_default();
            sink.lock().unwrap().push((domain, source.sequence));
            Ok((vec![command_to("inventory")], vec![]))
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
    assert_eq!(
        *seen.lock().unwrap(),
        vec![("order".to_string(), 4), ("order".to_string(), 6)]
    );
    let source_seqs: Vec<u32> = resp
        .commands
        .iter()
        .map(|c| cmd_page_deferred(&c.pages[0]).expect("deferred").source_seq)
        .collect();
    assert_eq!(
        source_seqs,
        vec![4, 6],
        "provenance matches what the handler saw"
    );
}

#[test]
fn a_context_handler_sees_sequence_zero_for_an_unsequenced_page() {
    let seen: Arc<Mutex<Vec<u32>>> = Arc::default();
    let sink = seen.clone();
    let saga = SagaDispatch::new("OrderFulfillment", "order", ["inventory"]).on_event_with_context(
        FQ_ORDER_CREATED,
        move |_e, _d, source| {
            sink.lock().unwrap().push(source.sequence);
            Ok((vec![], vec![]))
        },
    );
    saga.dispatch(&request(Some(source_book(
        "order",
        vec![event_page_of(FQ_ORDER_CREATED)],
    ))))
    .expect("dispatch");
    assert_eq!(*seen.lock().unwrap(), vec![0]);
}
