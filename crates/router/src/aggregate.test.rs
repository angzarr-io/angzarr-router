//! AggregateDispatch contracts, transliterated from client-go's
//! engine_test.go + engine_boundaries_test.go aggregate subset:
//! validate-before-rebuild, exact envelope guard codes, CommandContext
//! evidence, and fill-only ext/sequence stamping.

use std::sync::{Arc, Mutex};

use prost_types::Any;

use crate::aggregate::{AggregateDispatch, CommandContext};
use crate::error::codes;
use crate::pb;
use crate::test_support::*;
use crate::TYPE_URL_PREFIX;

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
    let cctx = contexts.lock().unwrap()[0];
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
    let cctx = contexts.lock().unwrap()[0];
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

// The dispatch path stamps the command's cover.ext onto the emitted
// EventBook's cover — FILL-ONLY, never overriding a handler-set ext.
// Emitted pages without headers get consecutive sequences from the
// aggregate's next sequence (fill-only).
#[test]
fn stamps_ext_and_sequences_fill_only() {
    let parent = cover_any("parent");
    let handler_ext = cover_any("handler-set");

    let explicit_ext = handler_ext.clone();
    let d = AggregateDispatch::new("agg", "order", fresh_rebuilder())
        .on_command("test.Create", |_, _: &mut TestState, _| {
            Ok(Some(pb::EventBook {
                pages: vec![pb::EventPage::default(), pb::EventPage::default()],
                ..Default::default()
            }))
        })
        .on_command("test.Explicit", move |_, _: &mut TestState, _| {
            Ok(Some(pb::EventBook {
                cover: Some(pb::Cover {
                    ext: Some(explicit_ext.clone()),
                    ..Default::default()
                }),
                pages: vec![pb::EventPage {
                    header: Some(pb::PageHeader {
                        sequence_type: Some(pb::page_header::SequenceType::Sequence(99)),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }))
        });

    let cmd_with = |fq_type: &str, ext: Option<Any>, next_seq: u32| {
        let mut req = pb::ContextualCommand {
            command: Some(pb::CommandBook {
                cover: Some(pb::Cover {
                    domain: "order".to_string(),
                    ext,
                    ..Default::default()
                }),
                pages: vec![pb::CommandPage {
                    payload: Some(pb::command_page::Payload::Command(Any {
                        type_url: format!("{TYPE_URL_PREFIX}{fq_type}"),
                        value: Vec::new(),
                    })),
                    ..Default::default()
                }],
            }),
            ..Default::default()
        };
        if next_seq > 0 {
            req.events = Some(pb::EventBook {
                next_sequence: next_seq,
                pages: vec![pb::EventPage::default()],
                ..Default::default()
            });
        }
        req
    };

    // Fill: command ext propagates to the emitted book's cover; headerless
    // pages get sequences next_seq, next_seq+1.
    let resp = d
        .dispatch(&cmd_with("test.Create", Some(parent.clone()), 5))
        .expect("dispatch");
    let events = events_of(&resp).expect("events");
    assert_eq!(
        events
            .cover
            .as_ref()
            .and_then(|c| c.ext.as_ref())
            .map(|e| e.type_url.as_str()),
        Some(parent.type_url.as_str()),
        "command cover.ext not stamped onto emitted book"
    );
    let seqs: Vec<u32> = events.pages.iter().map(crate::page_sequence).collect();
    assert_eq!(seqs, vec![5, 6], "fill-only sequence stamping");

    // Never override: handler-set ext and explicit headers survive.
    let resp = d
        .dispatch(&cmd_with("test.Explicit", Some(parent.clone()), 5))
        .expect("dispatch");
    let events = events_of(&resp).expect("events");
    assert_eq!(
        events.cover.as_ref().and_then(|c| c.ext.as_ref()),
        Some(&handler_ext),
        "handler-set ext was overridden"
    );
    assert_eq!(
        crate::page_sequence(&events.pages[0]),
        99,
        "explicit page header was overridden"
    );

    // No ext on the command → emitted cover.ext stays unset.
    let resp = d
        .dispatch(&cmd_with("test.Create", None, 0))
        .expect("dispatch");
    let events = events_of(&resp).expect("events");
    assert!(
        events.cover.as_ref().and_then(|c| c.ext.as_ref()).is_none(),
        "ext invented from nothing"
    );
}
