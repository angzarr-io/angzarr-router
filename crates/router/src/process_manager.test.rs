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
) -> pb::ProcessManagerHandleRequest {
    pb::ProcessManagerHandleRequest {
        trigger,
        process_state,
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
        ["inventory"],
        cover_applier(fresh_rebuilder()),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, _d, _cover| {
        Ok(pb::ProcessManagerHandleResponse {
            commands: vec![command_to("inventory")],
            ..Default::default()
        })
    })
}

/// A compensator response of process events plus an optional escalation.
fn pm_resp(
    process_events: Vec<pb::EventBook>,
    notification: Option<pb::Notification>,
) -> pb::ProcessManagerHandleResponse {
    pb::ProcessManagerHandleResponse {
        process_events,
        notification,
        ..Default::default()
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
        .dispatch(&request(Some(trigger(IN_DOMAIN, vec![ev(FQ_OTHER)])), None))
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
        ["inventory"],
        cover_applier(fresh_rebuilder()),
    )
    .on_event(
        IN_DOMAIN,
        FQ_SHIPPED,
        |_e, state: &mut TestState, _d, _cover| {
            let n = state.applied.len();
            Ok(pb::ProcessManagerHandleResponse {
                commands: (0..n).map(|_| command_to("inventory")).collect(),
                ..Default::default()
            })
        },
    );
    let state_book = book_of_covers(&["a", "b", "c"]);
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            Some(state_book),
        ))
        .expect("dispatch");
    assert_eq!(resp.commands.len(), 3, "three prior state events rebuilt");
}

#[test]
fn emitted_commands_are_deferred_from_the_trigger() {
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        ["inventory", "billing"],
        cover_applier(fresh_rebuilder()),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, dests, _cover| {
        assert_eq!(
            dests.domains(),
            ["inventory".to_string(), "billing".to_string()]
        );
        let mut explicit = command_to("inventory");
        explicit.pages[0].header = Some(pb::PageHeader {
            sequence_type: Some(pb::page_header::SequenceType::Sequence(9)),
            ..Default::default()
        });
        Ok(pb::ProcessManagerHandleResponse {
            commands: vec![explicit, command_to("billing")],
            ..Default::default()
        })
    });
    let mut newest = ev(FQ_SHIPPED);
    newest.header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(2)),
        ..Default::default()
    });
    let mut trig = trigger(IN_DOMAIN, vec![ev(FQ_OTHER), newest]);
    trig.cover.as_mut().unwrap().root = Some(pb::Uuid { value: vec![5] });
    let resp = d
        .dispatch(&request(Some(trig.clone()), None))
        .expect("dispatch");
    for (index, cmd) in resp.commands.iter().enumerate() {
        let Some(pb::page_header::SequenceType::AngzarrDeferred(dfr)) = cmd.pages[0]
            .header
            .as_ref()
            .and_then(|h| h.sequence_type.clone())
        else {
            panic!("command {index} is not deferred (C-0181)");
        };
        assert_eq!(dfr.source, trig.cover, "the trigger is the source");
        assert_eq!(dfr.source_seq, 2, "the newest trigger page's sequence");
        assert_eq!(dfr.command_index, index as u32);
    }
}

#[test]
fn accessors_report_the_declared_targets() {
    let d: ProcessManagerDispatch<TestState> = ProcessManagerDispatch::new(
        "pm",
        "pm-domain",
        ["inventory", "billing"],
        fresh_rebuilder(),
    );
    assert_eq!(
        d.target_domains(),
        ["inventory".to_string(), "billing".to_string()]
    );
}

#[test]
fn handler_can_emit_process_events_and_facts() {
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        ["inventory"],
        cover_applier(fresh_rebuilder()),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, _d, _cover| {
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
        ["inventory"],
        cover_applier(fresh_rebuilder()),
    )
    .on_rejected(FQ_RESERVE, |_n, _r, _s| {
        Ok(pm_resp(vec![tagged_book("comp-1")], None))
    })
    .on_rejected(FQ_RESERVE, |_n, _r, _s| {
        Ok(pm_resp(vec![tagged_book("comp-2")], None))
    });
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![notification_page_for(FQ_RESERVE)])),
            None,
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
        ["inventory"],
        cover_applier(fresh_rebuilder()),
    )
    .on_rejected(FQ_RESERVE, |_n, _r, _s| {
        Ok(pm_resp(vec![], Some(escalation("esc-1"))))
    })
    .on_rejected(FQ_RESERVE, |_n, _r, _s| {
        Ok(pm_resp(vec![], Some(escalation("esc-2"))))
    });
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![notification_page_for(FQ_RESERVE)])),
            None,
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
        .dispatch(&request(None, None))
        .expect_err("nil trigger must fail");
    assert_eq!(err.code, codes::MISSING_PM_TRIGGER);
}

#[test]
fn empty_trigger_is_empty_pm_trigger() {
    let d = pm_emitting_one();
    let err = d
        .dispatch(&request(Some(trigger(IN_DOMAIN, vec![])), None))
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
        ["inventory"],
        cover_applier(fresh_rebuilder()),
    )
    .on_rejected(FQ_RESERVE, |_n, _r, _s| Ok(pm_resp(vec![], None)));
    let err = d
        .dispatch(&request(Some(trigger(IN_DOMAIN, vec![bad])), None))
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
        ))
        .expect_err("corrupt state must fail");
    assert_eq!(err.code, codes::PERSISTED_EVENT_CORRUPT);
}

#[test]
fn handler_error_propagates_as_unhandled() {
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        ["inventory"],
        cover_applier(fresh_rebuilder()),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, _d, _cover| {
        Err(HandlerError::Other("boom".to_string()))
    });
    let err = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            None,
        ))
        .expect_err("handler error must fail dispatch");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
}

// --- accessors ------------------------------------------------------------

#[test]
fn accessors_report_name_domain_and_sources() {
    let rebuilder: Rebuilder<TestState> = fresh_rebuilder();
    let d = ProcessManagerDispatch::new("fulfillment-pm", "fulfillment", ["inventory"], rebuilder)
        .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, _d, _cover| {
            Ok(pb::ProcessManagerHandleResponse::default())
        })
        .on_event("billing", "test.Invoiced", |_e, _s, _d, _cover| {
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

// --- co-resident routing (select_process_managers / merge_response) --------

use crate::process_manager::{merge_response, select_process_managers, ProcessManagerRoute};

/// A PM named `name` owning `pm_domain`, consuming FQ_SHIPPED from `source`.
fn routed_pm(name: &str, pm_domain: &str, source: &str) -> ProcessManagerDispatch<TestState> {
    ProcessManagerDispatch::new(name, pm_domain, ["inventory"], fresh_rebuilder()).on_event(
        source,
        FQ_SHIPPED,
        |_e, _s, _d, _cover| Ok(pb::ProcessManagerHandleResponse::default()),
    )
}

/// A rejection Notification page whose rejected command carries an
/// angzarr_deferred header naming the issuing component and its domain.
fn issued_notification_page(source_component: &str, source_domain: &str) -> pb::EventPage {
    let rejection = pb::RejectionNotification {
        rejected_command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: "inventory".to_string(),
                ..Default::default()
            }),
            pages: vec![pb::CommandPage {
                header: Some(pb::PageHeader {
                    sequence_type: Some(pb::page_header::SequenceType::AngzarrDeferred(
                        pb::AngzarrDeferredSequence {
                            source: Some(pb::Cover {
                                domain: source_domain.to_string(),
                                ..Default::default()
                            }),
                            source_component: source_component.to_string(),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }),
                payload: Some(pb::command_page::Payload::Command(Any {
                    type_url: type_url(FQ_RESERVE),
                    value: Vec::new(),
                })),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    let notification = pb::Notification {
        payload: Some(Any {
            type_url: type_url("io.angzarr.v1.RejectionNotification"),
            value: prost::Message::encode_to_vec(&rejection),
        }),
        ..Default::default()
    };
    event_page(Any {
        type_url: type_url("io.angzarr.v1.Notification"),
        value: prost::Message::encode_to_vec(&notification),
    })
}

fn state_in(domain: &str) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn select(
    pms: &[&ProcessManagerDispatch<TestState>],
    req: &pb::ProcessManagerHandleRequest,
) -> Vec<usize> {
    let routes: Vec<&dyn ProcessManagerRoute> =
        pms.iter().map(|p| *p as &dyn ProcessManagerRoute).collect();
    select_process_managers(&routes, req)
}

#[test]
fn event_trigger_without_state_identity_selects_every_subscriber_in_order() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let other = routed_pm("x", "x-pm", "billing");
    let b = routed_pm("b", "b-pm", IN_DOMAIN);
    let req = request(Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])), None);
    assert_eq!(select(&[&a, &other, &b], &req), vec![0, 2]);
}

#[test]
fn event_trigger_with_uncovered_state_selects_subscribers() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let b = routed_pm("b", "b-pm", IN_DOMAIN);
    let req = request(
        Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
        Some(pb::EventBook::default()),
    );
    assert_eq!(select(&[&a, &b], &req), vec![0, 1]);
}

#[test]
fn process_state_cover_addresses_its_own_process_manager_only() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let b = routed_pm("b", "b-pm", IN_DOMAIN);
    let req = request(
        Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
        Some(state_in("b-pm")),
    );
    assert_eq!(select(&[&a, &b], &req), vec![1]);
}

#[test]
fn process_state_cover_matching_no_process_manager_falls_back_to_subscribers() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let b = routed_pm("b", "b-pm", "billing");
    let req = request(
        Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
        Some(state_in("unknown-pm")),
    );
    assert_eq!(select(&[&a, &b], &req), vec![0]);
}

#[test]
fn rejection_routes_to_the_issuing_component_by_name() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let b = routed_pm("b", "b-pm", IN_DOMAIN);
    // The trigger cover names a's domain, but the deferred provenance names b.
    let req = request(
        Some(trigger("a-pm", vec![issued_notification_page("b", "")])),
        None,
    );
    assert_eq!(select(&[&a, &b], &req), vec![1]);
}

#[test]
fn rejection_routes_to_the_issuing_domain_when_the_component_is_unknown() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let b = routed_pm("b", "b-pm", IN_DOMAIN);
    let req = request(
        Some(trigger(
            IN_DOMAIN,
            vec![issued_notification_page("gone", "b-pm")],
        )),
        None,
    );
    assert_eq!(select(&[&a, &b], &req), vec![1]);
}

#[test]
fn rejection_without_provenance_routes_by_the_trigger_cover_domain() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let b = routed_pm("b", "b-pm", IN_DOMAIN);
    let req = request(
        Some(trigger("a-pm", vec![notification_page_for(FQ_RESERVE)])),
        None,
    );
    assert_eq!(select(&[&a, &b], &req), vec![0]);
}

#[test]
fn unaddressed_rejection_never_fans_out_across_subscribers() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let b = routed_pm("b", "b-pm", IN_DOMAIN);
    let req = request(
        Some(trigger(IN_DOMAIN, vec![notification_page_for(FQ_RESERVE)])),
        None,
    );
    assert!(select(&[&a, &b], &req).is_empty());
}

#[test]
fn unaddressed_rejection_reaches_a_sole_process_manager() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let req = request(
        Some(trigger(IN_DOMAIN, vec![notification_page_for(FQ_RESERVE)])),
        None,
    );
    assert_eq!(select(&[&a], &req), vec![0]);
}

#[test]
fn undecodable_rejection_reaches_only_a_sole_process_manager() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let b = routed_pm("b", "b-pm", IN_DOMAIN);
    let garbage = event_page(Any {
        type_url: type_url("io.angzarr.v1.Notification"),
        value: vec![0xff, 0xff, 0xff],
    });
    let req = request(Some(trigger("a-pm", vec![garbage.clone()])), None);
    // The trigger cover still addresses a; the PM's own dispatch reports the
    // decode failure.
    assert_eq!(select(&[&a, &b], &req), vec![0]);
    let req = request(Some(trigger(IN_DOMAIN, vec![garbage])), None);
    assert!(select(&[&a, &b], &req).is_empty());
    assert_eq!(select(&[&a], &req), vec![0]);
}

#[test]
fn missing_or_empty_trigger_selects_nothing_to_route() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    assert!(select(&[&a], &request(None, None)).is_empty());
    assert!(select(&[&a], &request(Some(trigger(IN_DOMAIN, vec![])), None)).is_empty());
}

#[test]
fn route_view_reports_identity_and_consumption() {
    let a = routed_pm("a", "a-pm", IN_DOMAIN);
    let route: &dyn ProcessManagerRoute = &a;
    assert_eq!(route.name(), "a");
    assert_eq!(route.pm_domain(), "a-pm");
    assert!(route.consumes(IN_DOMAIN));
    assert!(!route.consumes("billing"));
}

#[test]
fn merge_concatenates_in_order_and_the_first_escalation_wins() {
    let mut acc = pb::ProcessManagerHandleResponse::default();
    merge_response(
        &mut acc,
        pb::ProcessManagerHandleResponse {
            process_events: vec![tagged_book("p1")],
            commands: vec![command_to("c1")],
            facts: vec![tagged_book("f1")],
            notification: None,
        },
    );
    merge_response(
        &mut acc,
        pb::ProcessManagerHandleResponse {
            process_events: vec![tagged_book("p2")],
            commands: vec![command_to("c2")],
            facts: vec![tagged_book("f2")],
            notification: Some(escalation("first")),
        },
    );
    merge_response(
        &mut acc,
        pb::ProcessManagerHandleResponse {
            notification: Some(escalation("second")),
            ..Default::default()
        },
    );
    assert_eq!(book_domains(&acc.process_events), vec!["p1", "p2"]);
    assert_eq!(book_domains(&acc.facts), vec!["f1", "f2"]);
    let cmd_domains: Vec<_> = acc
        .commands
        .iter()
        .map(|c| c.cover.as_ref().unwrap().domain.clone())
        .collect();
    assert_eq!(cmd_domains, vec!["c1", "c2"]);
    assert_eq!(acc.notification, Some(escalation("first")));
}

// --- compensation keys + Compensate payloads -------------------------------

/// A rejection Notification page for `fq` sent to `target_domain`.
fn rejection_sent_to(target_domain: &str, fq: &str) -> pb::EventPage {
    let mut page = notification_page_for(fq);
    let Some(pb::event_page::Payload::Event(any)) = page.payload.as_mut() else {
        unreachable!()
    };
    let mut notification: pb::Notification = prost::Message::decode(any.value.as_slice()).unwrap();
    let payload = notification.payload.as_mut().unwrap();
    let mut rejection: pb::RejectionNotification =
        prost::Message::decode(payload.value.as_slice()).unwrap();
    rejection
        .rejected_command
        .as_mut()
        .unwrap()
        .cover
        .as_mut()
        .unwrap()
        .domain = target_domain.to_string();
    payload.value = prost::Message::encode_to_vec(&rejection);
    any.value = prost::Message::encode_to_vec(&notification);
    page
}

fn compensate_page(command_type: &str) -> pb::EventPage {
    let notification = pb::Notification {
        payload: Some(Any {
            type_url: type_url("io.angzarr.v1.Compensate"),
            value: prost::Message::encode_to_vec(&pb::Compensate {
                command_type: command_type.to_string(),
                ..Default::default()
            }),
        }),
        ..Default::default()
    };
    event_page(Any {
        type_url: type_url("io.angzarr.v1.Notification"),
        value: prost::Message::encode_to_vec(&notification),
    })
}

fn labelled_compensator(
    pm: ProcessManagerDispatch<TestState>,
    key: &str,
    label: &'static str,
) -> ProcessManagerDispatch<TestState> {
    pm.on_rejected(key, move |_n, _r, _s| {
        Ok(pm_resp(vec![tagged_book(label)], None))
    })
}

#[test]
fn domain_qualified_compensator_matches_only_its_domain() {
    let pm = ProcessManagerDispatch::new("pm", "pm", ["inventory", "warehouse"], fresh_rebuilder());
    let pm = labelled_compensator(pm, &format!("inventory:{FQ_RESERVE}"), "from-inventory");
    let pm = labelled_compensator(pm, &format!("warehouse:{FQ_RESERVE}"), "from-warehouse");
    let resp = pm
        .dispatch(&request(
            Some(trigger(
                "pm",
                vec![rejection_sent_to("warehouse", FQ_RESERVE)],
            )),
            None,
        ))
        .expect("dispatch");
    assert_eq!(book_domains(&resp.process_events), vec!["from-warehouse"]);
    let resp = pm
        .dispatch(&request(
            Some(trigger(
                "pm",
                vec![rejection_sent_to("billing", FQ_RESERVE)],
            )),
            None,
        ))
        .expect("dispatch");
    assert!(resp.process_events.is_empty(), "no entry for billing");
}

#[test]
fn unqualified_compensator_matches_any_domain() {
    let pm = ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder());
    let pm = labelled_compensator(pm, FQ_RESERVE, "any");
    let resp = pm
        .dispatch(&request(
            Some(trigger(
                "pm",
                vec![rejection_sent_to("warehouse", FQ_RESERVE)],
            )),
            None,
        ))
        .expect("dispatch");
    assert_eq!(book_domains(&resp.process_events), vec!["any"]);
}

#[test]
fn validate_refuses_a_type_listed_both_ways() {
    let pm = ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder());
    let pm = labelled_compensator(pm, FQ_RESERVE, "a");
    assert!(pm.validate().is_ok());
    let pm = labelled_compensator(pm, &format!("inventory:{FQ_RESERVE}"), "b");
    assert_eq!(
        pm.validate().unwrap_err().code,
        codes::AMBIGUOUS_COMPENSATION
    );
}

#[test]
fn a_compensate_is_never_dropped_by_a_process_manager() {
    let pm = ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder());
    let pm = labelled_compensator(pm, FQ_RESERVE, "a");
    let err = pm
        .dispatch(&request(
            Some(trigger("pm", vec![compensate_page(FQ_RESERVE)])),
            None,
        ))
        .expect_err("undo is aggregate-only");
    assert_eq!(err.code, codes::NO_UNDO_HANDLER);
    assert_eq!(err.grpc, crate::error::GrpcCode::Unimplemented);
}

// A PM compensator may emit commands too (e.g. releasing a hold); they are
// deferred from the notification page like any PM command.
#[test]
fn compensator_commands_are_kept_and_deferred() {
    let pm = ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder()).on_rejected(
        FQ_RESERVE,
        |_n, _r, _s| {
            Ok(pb::ProcessManagerHandleResponse {
                commands: vec![command_to("inventory")],
                process_events: vec![tagged_book("released")],
                ..Default::default()
            })
        },
    );
    let mut page = notification_page_for(FQ_RESERVE);
    page.header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(6)),
        ..Default::default()
    });
    let resp = pm
        .dispatch(&request(Some(trigger("pm", vec![page])), None))
        .expect("dispatch");
    assert_eq!(book_domains(&resp.process_events), vec!["released"]);
    assert_eq!(resp.commands.len(), 1);
    let Some(pb::page_header::SequenceType::AngzarrDeferred(d)) = resp.commands[0].pages[0]
        .header
        .as_ref()
        .and_then(|h| h.sequence_type.clone())
    else {
        panic!("compensator command is not deferred");
    };
    assert_eq!(d.source_seq, 6);
    assert_eq!(d.source.map(|c| c.domain), Some("pm".to_string()));
}

// The handler reads the trigger's cover (e.g. to address the trigger's root).
#[test]
fn handler_sees_the_trigger_cover() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured = seen.clone();
    let pm = ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder()).on_event(
        IN_DOMAIN,
        FQ_SHIPPED,
        move |_e, _s, _d, cover| {
            *captured.lock().unwrap() = cover.cloned();
            Ok(pb::ProcessManagerHandleResponse::default())
        },
    );
    let mut trig = trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)]);
    trig.cover.as_mut().unwrap().root = Some(pb::Uuid { value: vec![3] });
    pm.dispatch(&request(Some(trig.clone()), None))
        .expect("dispatch");
    assert_eq!(*seen.lock().unwrap(), trig.cover);
}

#[test]
fn replay_folds_the_snapshot_then_the_events() {
    let pm = ProcessManagerDispatch::new(
        "pm",
        "pm",
        ["inventory"],
        cover_applier(fresh_rebuilder().with_snapshot(|s, _| {
            s.applied.push("snapshot".to_string());
            Ok(())
        })),
    );
    let state = pm
        .replay(&pb::ReplayRequest {
            base_snapshot: Some(pb::Snapshot {
                sequence: 1,
                state: Some(crate::test_support::cover_any("snap")),
                ..Default::default()
            }),
            events: book_of_covers(&["a", "b"]).pages,
        })
        .expect("replay");
    assert_eq!(state.applied, vec!["snapshot", "a", "b"]);
}

// --- process events address the PM's own stream (C-0477) -------------------

fn trigger_with_correlation(domain: &str, correlation_id: &str) -> pb::EventBook {
    let mut book = trigger(domain, vec![ev(FQ_SHIPPED)]);
    book.cover.as_mut().unwrap().correlation_id = correlation_id.to_string();
    book
}

fn process_state_at(domain: &str, root: &[u8]) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            root: Some(pb::Uuid {
                value: root.to_vec(),
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn pm_recording(process_events: fn() -> Vec<pb::EventBook>) -> ProcessManagerDispatch<TestState> {
    ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        ["inventory"],
        fresh_rebuilder(),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, move |_e, _s, _d, _cover| {
        Ok(pb::ProcessManagerHandleResponse {
            process_events: process_events(),
            ..Default::default()
        })
    })
    .on_rejected(FQ_RESERVE, move |_n, _r, _s| {
        Ok(pb::ProcessManagerHandleResponse {
            process_events: process_events(),
            ..Default::default()
        })
    })
}

#[test]
fn an_uncovered_process_event_is_addressed_to_the_pm_stream() {
    let d = pm_recording(|| vec![pb::EventBook::default()]);
    let resp = d
        .dispatch(&request(
            Some(trigger_with_correlation(IN_DOMAIN, "corr-7")),
            Some(process_state_at("fulfillment", &[7; 16])),
        ))
        .expect("dispatch");
    let cover = resp.process_events[0].cover.as_ref().expect("a cover");
    assert_eq!(cover.domain, "fulfillment");
    assert_eq!(
        cover.root.as_ref().map(|r| r.value.clone()),
        Some(vec![7; 16])
    );
    assert_eq!(cover.correlation_id, "corr-7");
}

#[test]
fn an_unaddressed_process_event_cover_is_filled_and_kept() {
    let d = pm_recording(|| {
        vec![pb::EventBook {
            cover: Some(pb::Cover {
                root: Some(pb::Uuid { value: vec![9; 16] }),
                correlation_id: "own".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }]
    });
    let resp = d
        .dispatch(&request(
            Some(trigger_with_correlation(IN_DOMAIN, "corr-7")),
            Some(process_state_at("fulfillment", &[7; 16])),
        ))
        .expect("dispatch");
    let cover = resp.process_events[0].cover.as_ref().expect("a cover");
    assert_eq!(cover.domain, "fulfillment", "the domain is filled");
    assert_eq!(
        cover.root.as_ref().map(|r| r.value.clone()),
        Some(vec![9; 16]),
        "the handler's root is kept"
    );
    assert_eq!(
        cover.correlation_id, "own",
        "the handler's correlation is kept"
    );
}

#[test]
fn an_addressed_process_event_is_left_alone() {
    let d = pm_recording(|| vec![tagged_book("elsewhere")]);
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            None,
        ))
        .expect("dispatch");
    assert_eq!(
        book_domains(&resp.process_events),
        vec!["elsewhere".to_string()]
    );
    assert_eq!(resp.process_events[0].cover.as_ref().unwrap().root, None);
}

#[test]
fn compensator_process_events_are_addressed_to_the_pm_stream() {
    let d = pm_recording(|| vec![pb::EventBook::default(), pb::EventBook::default()]);
    let resp = d
        .dispatch(&request(
            Some(trigger(
                "inventory",
                vec![notification_page_for(FQ_RESERVE)],
            )),
            None,
        ))
        .expect("dispatch");
    assert_eq!(
        book_domains(&resp.process_events),
        vec!["fulfillment".to_string(), "fulfillment".to_string()]
    );
}

#[test]
fn facts_are_not_readdressed() {
    let d = ProcessManagerDispatch::new(
        "fulfillment-pm",
        "fulfillment",
        ["inventory"],
        fresh_rebuilder(),
    )
    .on_event(IN_DOMAIN, FQ_SHIPPED, |_e, _s, _d, _cover| {
        Ok(pb::ProcessManagerHandleResponse {
            facts: vec![pb::EventBook::default()],
            ..Default::default()
        })
    });
    let resp = d
        .dispatch(&request(
            Some(trigger(IN_DOMAIN, vec![ev(FQ_SHIPPED)])),
            None,
        ))
        .expect("dispatch");
    assert_eq!(resp.facts[0].cover, None);
}
