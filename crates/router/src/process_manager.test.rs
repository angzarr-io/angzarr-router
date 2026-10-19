//! ProcessManagerDispatch contracts, transliterated from client-go's
//! engine.go ProcessManagerDispatch.Dispatch + features/process_manager.go
//! (spec C-0022, C-0042): a STATEFUL translator that fires only the newest
//! trigger page against rebuilt PM state, keys handlers by (input domain, FQ
//! type), treats out-of-sources / undeclared triggers as empty (not error),
//! and surfaces MISSING/EMPTY_PM_TRIGGER, MISSING_PM_EVENT_PAYLOAD and
//! PERSISTED_EVENT_CORRUPT (from rebuild).

use prost_types::Any;

use crate::error::{codes, HandlerError};
use crate::pb;
use crate::process_manager::ProcessManagerDispatch;
use crate::rebuild::Rebuilder;
use crate::test_support::{
    book_of_covers, corrupt_cover_any, cover_applier, event_page, fresh_rebuilder, TestState,
};
use crate::type_url;

const IN_DOMAIN: &str = "orders";
const FQ_SHIPPED: &str = "test.OrderShipped";
const FQ_OTHER: &str = "test.OrderCancelled";

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
fn merge_concatenates_in_order() {
    let mut acc = pb::ProcessManagerHandleResponse::default();
    merge_response(
        &mut acc,
        pb::ProcessManagerHandleResponse {
            process_events: vec![tagged_book("p1")],
            commands: vec![command_to("c1")],
            facts: vec![tagged_book("f1")],
            ..Default::default()
        },
    );
    merge_response(
        &mut acc,
        pb::ProcessManagerHandleResponse {
            process_events: vec![tagged_book("p2")],
            commands: vec![command_to("c2")],
            facts: vec![tagged_book("f2")],
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
