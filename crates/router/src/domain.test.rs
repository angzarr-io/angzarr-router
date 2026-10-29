//! DomainDispatch contracts, transliterated from client-go's
//! engine_test.go + engine_boundaries_test.go aggregate subset:
//! validate-before-rebuild, exact envelope guard codes, CommandContext
//! evidence, the Fact arm's ingest path, and pass-through of emitted
//! events.

use std::sync::{Arc, Mutex};

use prost::Message;
use prost_types::Any;

use crate::domain::{CommandContext, DomainDispatch, FactRecord};
use crate::error::{codes, GrpcCode};
use crate::pb;
use crate::test_support::*;
use crate::{page_event, type_url, TYPE_URL_PREFIX};

fn test_agg_dispatch() -> (DomainDispatch<TestState>, Arc<Mutex<Vec<CommandContext>>>) {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let seen = contexts.clone();
    let d = DomainDispatch::new("agg-test", "order", cover_applier(fresh_rebuilder())).on_command(
        &cover_full_name(),
        move |_, _, cctx| {
            seen.lock().unwrap().push(cctx);
            Ok(Some(pb::EventBook {
                pages: vec![pb::EventPage::default()],
                ..Default::default()
            }))
        },
    );
    (d, contexts)
}

fn events_of(resp: &pb::CommandHandlerResponse) -> Option<&pb::EventBook> {
    resp.events.as_ref()
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
    // No handler for Snapshot; corrupt prior events would surface
    // PERSISTED_EVENT_CORRUPT if the dispatcher rebuilt before validating
    // the command type.
    let mut cmd = command_for(any_of(&pb::Snapshot::default()));
    cmd.events = Some(pb::EventBook {
        pages: vec![event_page(corrupt_cover_any())],
        ..Default::default()
    });

    let err = d.dispatch(&cmd).expect_err("unknown command must fail");
    assert_eq!(err.code, "UNKNOWN_COMMAND_TYPE", "validate before rebuild");
}

// Spec: command_handler.proto, CommandHandlerService.Handle step 2 ("A type
// URL that reaches no registered handler is refused INVALID_ARGUMENT, naming
// the type URL"); decision 12.11; backlog VC-1.
#[test]
fn an_unknown_type_is_invalid_argument_naming_its_type_url() {
    let (d, _) = test_agg_dispatch();
    let url = type_url("test.Unknown");
    let unknown = Any {
        type_url: url.clone(),
        value: Vec::new(),
    };
    let err = d
        .dispatch(&command_for(unknown.clone()))
        .expect_err("an unknown command type");
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
    // Decision 14.37: the key is the status message; the args name the type.
    assert_eq!(err.code, "UNKNOWN_COMMAND_TYPE");
    assert_eq!(err.status_message(), "UNKNOWN_COMMAND_TYPE");
    assert_eq!(err.extras.get("type_url"), Some(&url));
    // upcaster.proto (VC-1): the ErrorInfo metadata key upcast_type_url
    // holds the type the chain ended at, the sent one when none applies.
    assert_eq!(
        err.extras.get("upcast_type_url").map(String::as_str),
        Some(url.as_str())
    );

    let err = d
        .dispatch(&contextual(fact_page(unknown)))
        .expect_err("an unknown fact type");
    assert_eq!(err.code, codes::NO_FACT_HANDLER);
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
    assert_eq!(err.status_message(), codes::NO_FACT_HANDLER);
    assert_eq!(err.extras.get("type_url"), Some(&url));
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
        err.code, "FOLD_FAILED",
        "never validate against truncated state; a failing fold is a handler bug"
    );
    assert_eq!(err.grpc, crate::error::GrpcCode::Internal);
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
        .expect_err("no page");
    assert_eq!(err.code, codes::MISSING_COMMAND_PAGE);
}

#[test]
fn page_without_payload_coded() {
    let (d, _) = test_agg_dispatch();
    let err = d
        .dispatch(&contextual(pb::CommandPage::default()))
        .expect_err("page without a body");
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
    let d = DomainDispatch::new("agg", "orders", fresh_rebuilder())
        .on_command("test.CreateOrder", |_, _: &mut TestState, _| {
            Ok(Some(pb::EventBook::default()))
        });
    assert_eq!(d.command_types(), vec!["test.CreateOrder".to_string()]);
}

// --- facts: the Fact arm of the command page ------------------------------

/// An aggregate folding Cover events whose Cover fact handler records the
/// fact annotated with the domains folded so far, flagged by one Cover event
/// naming the fact.
fn fact_aggregate() -> DomainDispatch<TestState> {
    DomainDispatch::new("agg-test", "ledger", cover_applier(fresh_rebuilder())).on_fact(
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

/// A fact delivery of a Cover fact named `name` over `prior` Cover events.
fn fact_delivery(name: &str, prior: &[&str]) -> pb::ContextualCommand {
    let mut req = contextual(fact_page(cover_any(name)));
    if !prior.is_empty() {
        req.events = Some(book_of_covers(prior));
    }
    req
}

#[test]
fn a_fact_is_recorded_then_flagged_against_the_folded_state() {
    let resp = fact_aggregate()
        .dispatch(&fact_delivery("f1", &["p", "q"]))
        .expect("fact");
    let book = resp.events.expect("events");
    assert_eq!(
        recorded_domains(&book),
        vec!["f1<p,q>", "flag:f1"],
        "the fact, then its flags, against the rebuilt state"
    );
    assert_eq!(book.cover, None, "the coordinator supplies the cover");
    assert!(
        book.pages
            .iter()
            .all(|p| p.meta.is_none() && p.sequence == 0),
        "payloads only: the coordinator stamps sequences and meta"
    );
}

#[test]
fn a_fact_recorded_as_received_needs_no_flags() {
    let d = DomainDispatch::new("agg-test", "ledger", fresh_rebuilder())
        .on_fact(&cover_full_name(), |fact, _s: &TestState| {
            Ok(FactRecord::from(fact.clone()))
        });
    let resp = d.dispatch(&fact_delivery("f1", &[])).expect("fact");
    assert_eq!(recorded_domains(&resp.events.unwrap()), vec!["f1"]);
}

#[test]
fn an_undeclared_fact_type_is_refused_before_any_handler_runs() {
    let ran = Arc::new(Mutex::new(0));
    let seen = ran.clone();
    let d = DomainDispatch::new("agg-test", "ledger", cover_applier(fresh_rebuilder())).on_fact(
        &cover_full_name(),
        move |fact, _s: &TestState| {
            *seen.lock().unwrap() += 1;
            Ok(FactRecord::from(fact.clone()))
        },
    );
    let mut req = contextual(fact_page(Any {
        type_url: type_url("test.Undeclared"),
        value: Vec::new(),
    }));
    // A corrupt history would surface PERSISTED_EVENT_CORRUPT had the
    // dispatcher rebuilt before refusing the fact.
    req.events = Some(pb::EventBook {
        pages: vec![event_page(corrupt_cover_any())],
        ..Default::default()
    });
    let err = d.dispatch(&req).expect_err("an undeclared fact type");
    assert_eq!(err.code, codes::NO_FACT_HANDLER);
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
    assert_eq!(
        err.extras.get("type_url").map(String::as_str),
        Some(type_url("test.Undeclared").as_str())
    );
    assert_eq!(*ran.lock().unwrap(), 0, "no fact handler ran");
}

// The body arm is the fact marker: an arm that does not match the
// registered handler's kind is refused, never run by the other kind.
#[test]
fn a_command_type_sent_as_a_fact_is_refused() {
    let (d, contexts) = test_agg_dispatch();
    let err = d
        .dispatch(&contextual(fact_page(cover_any("x"))))
        .expect_err("a command type in the Fact arm");
    assert_eq!(err.code, codes::NO_FACT_HANDLER);
    assert!(
        contexts.lock().unwrap().is_empty(),
        "no command handler ran"
    );
}

#[test]
fn a_fact_type_sent_as_a_command_is_refused() {
    let err = fact_aggregate()
        .dispatch(&command_for(cover_any("f1")))
        .expect_err("a fact type in the Command arm");
    assert_eq!(err.code, "UNKNOWN_COMMAND_TYPE");
}

#[test]
fn a_fact_without_a_payload_is_coded() {
    let err = fact_aggregate()
        .dispatch(&contextual(pb::CommandPage {
            body: Some(pb::command_page::Body::Fact(pb::Fact { payload: None })),
            ..Default::default()
        }))
        .expect_err("a fact with no payload");
    assert_eq!(err.code, codes::MISSING_COMMAND_PAYLOAD);
}

#[test]
fn a_corrupt_prior_event_fails_the_fact() {
    let mut req = fact_delivery("f1", &[]);
    req.events = Some(pb::EventBook {
        pages: vec![event_page(corrupt_cover_any())],
        ..Default::default()
    });
    let err = fact_aggregate()
        .dispatch(&req)
        .expect_err("corrupt history");
    // Decision 14.37: its own key, not UNHANDLED_HANDLER_ERROR.
    assert_eq!(err.code, "FOLD_FAILED");
}

#[test]
fn a_failing_fact_handler_fails_the_request() {
    let d = DomainDispatch::new("agg-test", "ledger", fresh_rebuilder())
        .on_fact(&cover_full_name(), |_f, _s: &TestState| {
            Err(crate::error::HandlerError::Other("broken".to_string()))
        });
    let err = d
        .dispatch(&fact_delivery("f1", &[]))
        .expect_err("handler error");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
}

// Decision 14.37: a handler that panics is a bug, UNHANDLED_HANDLER_ERROR
// with cause HANDLER_PANICKED; the panic's message stays in the handler's
// own log and the router answers, rather than unwinding into its host.
#[test]
fn a_panicking_handler_is_a_bug_caused_by_handler_panicked() {
    let d = DomainDispatch::new("agg-test", "order", cover_applier(fresh_rebuilder()))
        .on_command(&cover_full_name(), |_, _, _| panic!("index out of bounds"));
    let req = command_for(cover_any("a"));
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.dispatch(&req)));
    let err = match answer {
        Ok(result) => result.expect_err("a panic is no success"),
        Err(_) => panic!("the handler's panic escaped the router: it answers HANDLER_PANICKED"),
    };
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(
        err.extras.get("cause").map(String::as_str),
        Some("HANDLER_PANICKED")
    );
    assert!(!format!("{err:?}").contains("index out of bounds"));
}

#[test]
fn fact_types_exact() {
    assert_eq!(fact_aggregate().fact_types(), vec![cover_full_name()]);
}

// The emitted EventBook passes through exactly as the handler returned it:
// the coordinator stamps the cover, sequences and meta (and refuses output
// that sets them), so the router adds nothing and polices nothing.
#[test]
fn the_emitted_book_passes_through_unstamped() {
    let emitted = pb::EventBook {
        pages: vec![
            pb::EventPage::default(),
            pb::EventPage {
                sequence: 99,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let returned = emitted.clone();
    let d = DomainDispatch::new("agg", "order", fresh_rebuilder())
        .on_command("test.Create", move |_, _: &mut TestState, _| {
            Ok(Some(returned.clone()))
        });
    let mut req = command_for(Any {
        type_url: format!("{TYPE_URL_PREFIX}test.Create"),
        value: Vec::new(),
    });
    req.command.as_mut().unwrap().cover = Some(pb::Cover {
        domain: "order".to_string(),
        ..Default::default()
    });
    req.events = Some(pb::EventBook {
        next_sequence: 5,
        pages: vec![pb::EventPage::default()],
        ..Default::default()
    });
    let resp = d.dispatch(&req).expect("dispatch");
    assert_eq!(resp.events, Some(emitted));
    assert_eq!(resp.rules_version, "");
}

#[test]
fn a_handler_emitting_nothing_answers_an_empty_book() {
    let d = DomainDispatch::new("agg", "order", fresh_rebuilder())
        .on_command("test.Create", |_, _: &mut TestState, _| Ok(None));
    let resp = d
        .dispatch(&command_for(Any {
            type_url: type_url("test.Create"),
            value: Vec::new(),
        }))
        .expect("dispatch");
    assert_eq!(resp.events, Some(pb::EventBook::default()));
}
