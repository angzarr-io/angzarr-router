//! AggregateDispatch contracts, transliterated from client-go's
//! engine_test.go + engine_boundaries_test.go aggregate subset:
//! validate-before-rebuild, exact envelope guard codes, CommandContext
//! evidence, FQ-keyed rejection routing with ordered fan-out, and
//! fill-only ext/sequence stamping.

use std::sync::{Arc, Mutex};

use prost::Message;
use prost_types::Any;

use crate::aggregate::{AggregateDispatch, CommandContext, FactRecord};
use crate::error::{codes, GrpcCode};
use crate::pb;
use crate::test_support::*;
use crate::{page_event, type_url, TYPE_URL_PREFIX};

fn test_agg_dispatch() -> (
    AggregateDispatch<TestState>,
    Arc<Mutex<Vec<CommandContext>>>,
) {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let seen = contexts.clone();
    let d = AggregateDispatch::new("agg-test", "order", cover_applier(fresh_rebuilder()))
        .on_command(&cover_full_name(), move |_, _, cctx| {
            seen.lock().unwrap().push(cctx);
            Ok(Some(pb::EventBook {
                pages: vec![pb::EventPage::default()],
                ..Default::default()
            }))
        });
    (d, contexts)
}

fn events_of(resp: &pb::BusinessResponse) -> Option<&pb::EventBook> {
    match &resp.result {
        Some(pb::business_response::Result::Events(book)) => Some(book),
        _ => None,
    }
}

#[test]
fn absent_events_fresh_state_no_prior_history() {
    let (d, contexts) = test_agg_dispatch();
    let resp = d.dispatch(&command_for(cover_any(""))).expect("dispatch");
    assert!(events_of(&resp).is_some(), "expected events result");
    let cctx = contexts.lock().unwrap()[0].clone();
    assert!(
        !cctx.had_prior_events,
        "had_prior_events for absent events (the Exists() bug)"
    );
    assert_eq!(cctx.next_sequence, 0);
}

#[test]
fn prior_events_reach_state_and_context() {
    let (d, contexts) = test_agg_dispatch();
    let mut cmd = command_for(cover_any(""));
    let mut prior = book_of_covers(&["p1", "p2"]);
    prior.next_sequence = 2;
    cmd.events = Some(prior);

    d.dispatch(&cmd).expect("dispatch");
    let cctx = contexts.lock().unwrap()[0].clone();
    assert!(cctx.had_prior_events, "had_prior_events with 2 prior pages");
    assert_ne!(
        cctx.next_sequence, 0,
        "next_sequence not derived from prior events"
    );
}

#[test]
fn unknown_command_coded_before_rebuild() {
    let (d, _) = test_agg_dispatch();
    // No handler for Edition; corrupt prior events would surface
    // PERSISTED_EVENT_CORRUPT if the dispatcher rebuilt before validating
    // the command type.
    let mut cmd = command_for(any_of(&pb::Edition::default()));
    cmd.events = Some(pb::EventBook {
        pages: vec![event_page(corrupt_cover_any())],
        ..Default::default()
    });

    let err = d.dispatch(&cmd).expect_err("unknown command must fail");
    assert_eq!(
        err.code,
        codes::NO_HANDLER_REGISTERED,
        "validate before rebuild"
    );
}

#[test]
fn corrupt_prior_event_fails_command() {
    let (d, _) = test_agg_dispatch();
    let mut cmd = command_for(cover_any(""));
    cmd.events = Some(pb::EventBook {
        pages: vec![event_page(corrupt_cover_any())],
        ..Default::default()
    });

    let err = d.dispatch(&cmd).expect_err("corrupt prior event must fail");
    assert_eq!(
        err.code,
        codes::PERSISTED_EVENT_CORRUPT,
        "never validate against truncated state"
    );
}

#[test]
fn missing_command_envelope_guards_exact_codes() {
    let (d, _) = test_agg_dispatch();

    let err = d
        .dispatch(&pb::ContextualCommand::default())
        .expect_err("no command book");
    assert_eq!(err.code, codes::MISSING_COMMAND_BOOK);

    let err = d
        .dispatch(&pb::ContextualCommand {
            command: Some(pb::CommandBook::default()),
            ..Default::default()
        })
        .expect_err("empty pages");
    assert_eq!(err.code, codes::MISSING_COMMAND_PAGE);
}

#[test]
fn page_without_payload_coded() {
    let (d, _) = test_agg_dispatch();
    let err = d
        .dispatch(&pb::ContextualCommand {
            command: Some(pb::CommandBook {
                pages: vec![pb::CommandPage::default()],
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect_err("page without payload");
    assert_eq!(err.code, codes::MISSING_COMMAND_PAYLOAD);
}

#[test]
fn empty_type_url_coded() {
    let (d, _) = test_agg_dispatch();
    let err = d
        .dispatch(&command_for(Any::default()))
        .expect_err("empty type_url");
    assert_eq!(err.code, codes::MISSING_COMMAND_PAYLOAD);
}

#[test]
fn name_and_domain_are_exact() {
    let (d, _) = test_agg_dispatch();
    assert_eq!(d.name(), "agg-test");
    assert_eq!(d.domain(), "order");
}

#[test]
fn command_types_exact() {
    let d = AggregateDispatch::new("agg", "orders", fresh_rebuilder())
        .on_command("test.CreateOrder", |_, _: &mut TestState, _| {
            Ok(Some(pb::EventBook::default()))
        });
    assert_eq!(d.command_types(), vec!["test.CreateOrder".to_string()]);
}

// --- compensation stamping, qualified keys, undo (Compensate) --------------

// --- facts and replay ------------------------------------------------------

fn fact_page(domain: &str) -> pb::EventPage {
    pb::EventPage {
        header: Some(pb::PageHeader {
            sequence_type: Some(pb::page_header::SequenceType::ExternalDeferred(
                pb::ExternalDeferredSequence {
                    external_id: format!("ext-{domain}"),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }),
        ..event_page(cover_any(domain))
    }
}

/// An aggregate folding Cover events whose Cover fact handler records the
/// fact annotated with the domains folded so far, flagged by one Cover event
/// naming the fact.
fn fact_aggregate() -> AggregateDispatch<TestState> {
    AggregateDispatch::new("agg-test", "ledger", cover_applier(fresh_rebuilder())).on_fact(
        &cover_full_name(),
        |fact, state: &TestState| {
            let folded = pb::Cover::decode(fact.value.as_slice()).unwrap().domain;
            Ok(FactRecord {
                fact: cover_any(&format!("{folded}<{}>", state.applied.join(","))),
                flags: vec![cover_any(&format!("flag:{folded}"))],
            })
        },
    )
}

fn recorded_domains(book: &pb::EventBook) -> Vec<String> {
    book.pages
        .iter()
        .map(|p| match page_event(p) {
            Some(any) => pb::Cover::decode(any.value.as_slice()).unwrap().domain,
            None => String::new(),
        })
        .collect()
}

fn ledger_facts(pages: Vec<pb::EventPage>) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: "ledger".to_string(),
            ..Default::default()
        }),
        pages,
        ..Default::default()
    }
}

#[test]
fn each_fact_is_recorded_then_flagged_and_every_recorded_event_folds() {
    let mut first = fact_page("f1");
    first.created_at = Some(prost_types::Timestamp {
        seconds: 42,
        nanos: 0,
    });
    let facts = ledger_facts(vec![first, fact_page("f2")]);
    let out = fact_aggregate()
        .handle_fact(&pb::FactRequest {
            facts: Some(facts.clone()),
            prior_events: Some(book_of_covers(&["p"])),
        })
        .expect("facts");
    assert_eq!(out.cover, facts.cover, "the facts' cover is kept");
    assert_eq!(
        recorded_domains(&out),
        vec!["f1<p>", "flag:f1", "f2<p,f1<p>,flag:f1>", "flag:f2"],
        "each fact, then its flags; each folds before the next fact"
    );
    assert_eq!(
        out.pages[0].header, facts.pages[0].header,
        "a fact keeps its header"
    );
    assert_eq!(out.pages[2].header, facts.pages[1].header);
    assert_eq!(out.pages[1].header, None, "a flag carries no header");
    assert_eq!(
        out.pages[1].created_at, out.pages[0].created_at,
        "a flag is stamped when its fact was"
    );
    assert_eq!(out.pages[3].created_at, None);
}

#[test]
fn a_fact_recorded_as_received_needs_no_flags() {
    let d = AggregateDispatch::new("agg-test", "ledger", fresh_rebuilder())
        .on_fact(&cover_full_name(), |fact, _s: &TestState| {
            Ok(FactRecord::from(fact.clone()))
        });
    let facts = ledger_facts(vec![fact_page("f1")]);
    let out = d
        .handle_fact(&pb::FactRequest {
            facts: Some(facts.clone()),
            prior_events: None,
        })
        .expect("facts");
    assert_eq!(out.pages, facts.pages);
}

#[test]
fn an_undeclared_fact_type_is_refused_before_any_handler_runs() {
    let ran = std::sync::Arc::new(std::sync::Mutex::new(0));
    let seen = ran.clone();
    let d = AggregateDispatch::new("agg-test", "ledger", fresh_rebuilder()).on_fact(
        &cover_full_name(),
        move |fact, _s: &TestState| {
            *seen.lock().unwrap() += 1;
            Ok(FactRecord::from(fact.clone()))
        },
    );
    let undeclared = event_page(Any {
        type_url: type_url("test.Undeclared"),
        value: Vec::new(),
    });
    let err = d
        .handle_fact(&pb::FactRequest {
            facts: Some(ledger_facts(vec![fact_page("f1"), undeclared])),
            prior_events: None,
        })
        .expect_err("an undeclared fact type");
    assert_eq!(err.code, codes::NO_FACT_HANDLER);
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
    assert_eq!(
        err.extras.get("type_url").map(String::as_str),
        Some(type_url("test.Undeclared").as_str())
    );
    assert_eq!(*ran.lock().unwrap(), 0, "no fact handler ran");
}

#[test]
fn a_fact_page_without_an_event_is_recorded_unchanged() {
    let d = AggregateDispatch::new("agg-test", "ledger", fresh_rebuilder());
    let facts = ledger_facts(vec![pb::EventPage {
        header: fact_page("f1").header,
        ..Default::default()
    }]);
    let out = d
        .handle_fact(&pb::FactRequest {
            facts: Some(facts.clone()),
            prior_events: None,
        })
        .expect("facts");
    assert_eq!(out.pages, facts.pages);
}

#[test]
fn absent_facts_record_nothing() {
    let out = fact_aggregate()
        .handle_fact(&pb::FactRequest {
            facts: None,
            prior_events: None,
        })
        .expect("facts");
    assert_eq!(out, pb::EventBook::default());
}

#[test]
fn a_failing_fact_handler_fails_the_request() {
    let d = AggregateDispatch::new("agg-test", "ledger", fresh_rebuilder())
        .on_fact(&cover_full_name(), |_f, _s: &TestState| {
            Err(crate::error::HandlerError::Other("broken".to_string()))
        });
    let err = d
        .handle_fact(&pb::FactRequest {
            facts: Some(pb::EventBook {
                pages: vec![fact_page("f1")],
                ..Default::default()
            }),
            prior_events: None,
        })
        .expect_err("handler error");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
}

#[test]
fn replay_folds_the_snapshot_then_the_events() {
    let d = AggregateDispatch::new(
        "agg-test",
        "ledger",
        cover_applier(fresh_rebuilder().with_snapshot(|s, _| {
            s.applied.push("snapshot".to_string());
            Ok(())
        })),
    );
    let state = d
        .replay(&pb::ReplayRequest {
            base_snapshot: Some(pb::Snapshot {
                sequence: 1,
                state: Some(cover_any("snap")),
                ..Default::default()
            }),
            events: book_of_covers(&["a", "b"]).pages,
        })
        .expect("replay");
    assert_eq!(state.applied, vec!["snapshot", "a", "b"]);
}

// The emitted EventBook passes through exactly as the handler returned it:
// the coordinator stamps sequences and meta, so the router adds no header and
// copies nothing from the command's cover, and a handler-set header survives.
#[test]
fn the_emitted_book_passes_through_unstamped() {
    let emitted = pb::EventBook {
        pages: vec![
            pb::EventPage::default(),
            pb::EventPage {
                header: Some(pb::PageHeader {
                    sequence_type: Some(pb::page_header::SequenceType::Sequence(99)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let returned = emitted.clone();
    let d = AggregateDispatch::new("agg", "order", fresh_rebuilder())
        .on_command("test.Create", move |_, _: &mut TestState, _| {
            Ok(Some(returned.clone()))
        });
    let mut req = command_for(Any {
        type_url: format!("{TYPE_URL_PREFIX}test.Create"),
        value: Vec::new(),
    });
    let cover = req
        .command
        .as_mut()
        .unwrap()
        .cover
        .get_or_insert_with(Default::default);
    cover.domain = "order".to_string();
    cover.ext = Some(cover_any("parent"));
    req.events = Some(pb::EventBook {
        next_sequence: 5,
        pages: vec![pb::EventPage::default()],
        ..Default::default()
    });
    let resp = d.dispatch(&req).expect("dispatch");
    assert_eq!(events_of(&resp), Some(&emitted));
}
