//! ProcessManagerDispatch contracts, transliterated from client-go's
//! engine.go ProcessManagerDispatch.Dispatch + features/process_manager.go
//! (spec C-0022, C-0042): a STATEFUL translator that fires only the newest
//! trigger page against rebuilt PM state, keys handlers by (input domain, FQ
//! type), treats out-of-sources / undeclared triggers as empty (not error),
//! routes Notification triggers to ordered compensators (first escalation
//! wins), and surfaces MISSING/EMPTY_PM_TRIGGER, MISSING_PM_EVENT_PAYLOAD,
//! NOTIFICATION_DECODE_FAILED, and PERSISTED_EVENT_CORRUPT (from rebuild).

use prost_types::Any;

use crate::error::{codes, HandlerError};
use crate::pb;
use crate::process_manager::ProcessManagerDispatch;
use crate::rebuild::Rebuilder;
use crate::test_support::{
    book_of_covers, corrupt_cover_any, cover_applier, event_page, fresh_rebuilder,
    notification_page_for, TestState,
};
use crate::type_url;

const IN_DOMAIN: &str = "orders";
const FQ_SHIPPED: &str = "test.OrderShipped";
const FQ_OTHER: &str = "test.OrderCancelled";
const FQ_RESERVE: &str = "test.ReserveStock";

// --- fixtures -------------------------------------------------------------

/// A PM request over an optional trigger / process-state and a destination
/// map.
fn request(
    trigger: Option<pb::EventBook>,
    process_state: Option<pb::EventBook>,
    dest: &[(&str, u32)],
) -> pb::ProcessManagerHandleRequest {
    pb::ProcessManagerHandleRequest {
        trigger,
        process_state,
        destination_sequences: dest.iter().map(|(d, s)| (d.to_string(), *s)).collect(),
    }
}

/// A trigger EventBook in `domain` over the given pages.
fn trigger(domain: &str, pages: Vec<pb::EventPage>) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages,
        ..Default::default()
    }
}

/// An event page carrying a bare Any of the fully-qualified type.
fn ev(fq: &str) -> pb::EventPage {
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

/// A process-event / fact EventBook tagged with `label` in its cover domain.
fn tagged_book(label: &str) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: label.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// An escalation Notification tagged with `label` in its cover domain.
fn escalation(label: &str) -> pb::Notification {
    pb::Notification {
        cover: Some(pb::Cover {
            domain: label.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A handler that emits one command to "inventory" (the simplest reaction).
fn pm_emitting_one() -> ProcessManagerDispatch<TestState> {
    ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        cover_applier(fresh_rebuilder()),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, _d| {
        Ok(pb::ProcessManagerHandleResponse {
            commands: vec![command_to("inventory")],
            ..Default::default()
        })
    })
}

fn cmd_page_seq(page: &pb::CommandPage) -> Option<u32> {
    match page.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
        Some(pb::page_header::SequenceType::Sequence(s)) => Some(*s),
        _ => None,
    }
}

fn book_domains(books: &[pb::EventBook]) -> Vec<String> {
    books
        .iter()
        .map(|b| {
            b.cover
                .as_ref()
                .map(|c| c.domain.clone())
                .unwrap_or_default()
        })
        .collect()
}

// --- trigger routing + newest-page semantics ------------------------------

#[test]
fn newest_declared_event_runs_handler() {
    let d = pm_emitting_one();
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            None,
            &[],
        ))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 1);
    assert_eq!(resp.commands[0].cover.as_ref().unwrap().domain, "inventory");
}

#[test]
fn only_the_newest_page_fires() {
    // Pages [declared, undeclared]: the newest (undeclared) decides — the
    // declared page-0 must NOT re-fire from history.
    let d = pm_emitting_one();
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED), ev(FQ_OTHER)])),
            None,
            &[],
        ))
        .expect("dispatch");
    assert!(resp.commands.is_empty(), "history must not re-trigger");
}

#[test]
fn newest_page_fires_even_after_undeclared_history() {
    // Pages [undeclared, declared]: the newest (declared) fires.
    let d = pm_emitting_one();
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_OTHER), ev(FQ_SHIPPED)])),
            None,
            &[],
        ))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 1, "newest page triggers");
}

#[test]
fn trigger_outside_sources_is_empty() {
    let d = pm_emitting_one();
    let resp = d
        .dispatch(&request(
            Some(trigger("unrelated", vec![ev(FQ_SHIPPED)])),
            None,
            &[],
        ))
        .expect("dispatch");
    assert!(
        resp.commands.is_empty(),
        "domain outside sources → empty (C-0022)"
    );
}

#[test]
fn undeclared_event_type_is_empty() {
    let d = pm_emitting_one();
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_OTHER)])),
            None,
            &[],
        ))
        .expect("dispatch");
    assert!(resp.commands.is_empty(), "undeclared type → empty");
}

// --- stateful rebuild + destinations --------------------------------------

#[test]
fn handler_sees_rebuilt_state() {
    // The handler emits one command per prior PM state event — proving the
    // process_state was rebuilt and handed in.
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        cover_applier(fresh_rebuilder()),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, state: &mut TestState, _d| {
        let n = state.applied.len();
        Ok(pb::ProcessManagerHandleResponse {
            commands: (0..n).map(|_| command_to("inventory")).collect(),
            ..Default::default()
        })
    });
    let state_book = book_of_covers(&["a", "b", "c"]);
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            Some(state_book),
            &[],
        ))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 3, "three prior state events rebuilt");
}

#[test]
fn handler_stamps_from_destinations() {
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        cover_applier(fresh_rebuilder()),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, dests| {
        let mut cmd = command_to("inventory");
        dests.stamp_command(&mut cmd, "inventory")?;
        Ok(pb::ProcessManagerHandleResponse {
            commands: vec![cmd],
            ..Default::default()
        })
    });
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            None,
            &[("inventory", 9)],
        ))
        .expect("dispatch");
    assert_eq!(cmd_page_seq(&resp.commands[0].pages[0]), Some(9));
}

#[test]
fn handler_can_emit_process_events_and_facts() {
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        cover_applier(fresh_rebuilder()),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, _d| {
        Ok(pb::ProcessManagerHandleResponse {
            process_events: vec![tagged_book("pe")],
            facts: vec![tagged_book("fact")],
            ..Default::default()
        })
    });
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            None,
            &[],
        ))
        .expect("dispatch");
    assert_eq!(book_domains(&resp.process_events), vec!["pe".to_string()]);
    assert_eq!(book_domains(&resp.facts), vec!["fact".to_string()]);
}

// --- rejection fan-out (C-0042) + escalation ------------------------------

#[test]
fn notification_routes_to_ordered_compensators() {
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        cover_applier(fresh_rebuilder()),
    )
    .on_rejected(FQ_RESERVE, |_n, _r, _s| {
        Ok((vec![tagged_book("comp-1")], None))
    })
    .on_rejected(FQ_RESERVE, |_n, _r, _s| {
        Ok((vec![tagged_book("comp-2")], None))
    });
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![notification_page_for(FQ_RESERVE)])),
            None,
            &[],
        ))
        .expect("dispatch");
    assert_eq!(
        book_domains(&resp.process_events),
        vec!["comp-1".to_string(), "comp-2".to_string()]
    );
}

#[test]
fn first_escalation_wins() {
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        cover_applier(fresh_rebuilder()),
    )
    .on_rejected(FQ_RESERVE, |_n, _r, _s| {
        Ok((vec![], Some(escalation("esc-1"))))
    })
    .on_rejected(FQ_RESERVE, |_n, _r, _s| {
        Ok((vec![], Some(escalation("esc-2"))))
    });
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![notification_page_for(FQ_RESERVE)])),
            None,
            &[],
        ))
        .expect("dispatch");
    assert_eq!(
        resp.notification.expect("escalation").cover.unwrap().domain,
        "esc-1",
        "first escalation wins"
    );
}

#[test]
fn undeclared_rejection_yields_empty_response() {
    let d = pm_emitting_one(); // no compensators registered
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![notification_page_for(FQ_RESERVE)])),
            None,
            &[],
        ))
        .expect("dispatch");
    assert!(resp.process_events.is_empty());
    assert!(resp.notification.is_none());
}

// --- envelope + error guards ----------------------------------------------

#[test]
fn nil_trigger_is_missing_pm_trigger() {
    let d = pm_emitting_one();
    let err = d
        .dispatch(&request(None, None, &[]))
        .expect_err("nil trigger must fail");
    assert_eq!(err.code, codes::MISSING_PM_TRIGGER);
}

#[test]
fn empty_trigger_is_empty_pm_trigger() {
    let d = pm_emitting_one();
    let err = d
        .dispatch(&request(Some(trigger(IN_DOMAIN, vec![])), None, &[]))
        .expect_err("empty trigger must fail");
    assert_eq!(err.code, codes::EMPTY_PM_TRIGGER);
}

#[test]
fn trigger_last_page_without_payload_is_coded() {
    let d = pm_emitting_one();
    let err = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![pb::EventPage::default()])),
            None,
            &[],
        ))
        .expect_err("payload-less trigger must fail");
    assert_eq!(err.code, codes::MISSING_PM_EVENT_PAYLOAD);
}

#[test]
fn corrupt_notification_payload_is_coded() {
    let bad = event_page(Any {
        type_url: crate::NOTIFICATION_TYPE_URL.to_string(),
        value: vec![0xFF, 0xFF, 0xFF, 0xFF],
    });
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        cover_applier(fresh_rebuilder()),
    )
    .on_rejected(FQ_RESERVE, |_n, _r, _s| Ok((vec![], None)));
    let err = d
        .dispatch(&request(Some(trigger(IN_DOMAIN, vec![bad])), None, &[]))
        .expect_err("corrupt notification must fail");
    assert_eq!(err.code, codes::NOTIFICATION_DECODE_FAILED);
}

#[test]
fn corrupt_process_state_is_data_loss() {
    // A corrupt prior PM-state event fails the rebuild before the handler runs.
    let state = pb::EventBook {
        pages: vec![event_page(corrupt_cover_any())],
        ..Default::default()
    };
    let d = pm_emitting_one();
    let err = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            Some(state),
            &[],
        ))
        .expect_err("corrupt state must fail");
    assert_eq!(err.code, codes::PERSISTED_EVENT_CORRUPT);
}

#[test]
fn handler_error_propagates_as_unhandled() {
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        cover_applier(fresh_rebuilder()),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, _d| {
        Err(HandlerError::Other("boom".to_string()))
    });
    let err = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            None,
            &[],
        ))
        .expect_err("handler error must fail dispatch");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
}

// --- accessors ------------------------------------------------------------

#[test]
fn accessors_report_name_domain_and_sources() {
    let rebuilder: Rebuilder<TestState> = fresh_rebuilder();
    let d = ProcessManagerDispatch::new("fulfillment-pm", "fulfillment", rebuilder)
        .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, _d| {
            Ok(pb::ProcessManagerHandleResponse::default())
        })
        .on_event("billing", "test.Invoiced", |_e, _s, _d| {
            Ok(pb::ProcessManagerHandleResponse::default())
        });
    assert_eq!(d.name(), "fulfillment-pm");
    assert_eq!(d.pm_domain(), "fulfillment");
    let mut sources = d.sources();
    sources.sort();
    assert_eq!(sources, vec!["billing".to_string(), "orders".to_string()]);
    assert_eq!(
        d.subscriptions().get(IN_DOMAIN),
        Some(&vec![FQ_SHIPPED.to_string()])
    );
}
