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

const FQ_RESERVE: &str = "io.angzarr.examples.v1.ReserveStock";

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

fn notification_command_for(fq_command: &str) -> Any {
    page_event(&notification_page_for(fq_command))
        .expect("notification page carries an event")
        .clone()
}

#[test]
fn rejection_routes_by_fq_with_state() {
    let saw_state = Arc::new(Mutex::new(None));
    let seen = saw_state.clone();
    let d = AggregateDispatch::new("agg-test", "order", cover_applier(fresh_rebuilder()))
        .on_rejected(FQ_RESERVE, move |_, _, state: &mut TestState, _| {
            *seen.lock().unwrap() = Some(state.applied.clone());
            Ok(pb::BusinessResponse::default())
        });

    let cmd = pb::ContextualCommand {
        command: Some(pb::CommandBook {
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(
                    notification_command_for(FQ_RESERVE),
                )),
                ..Default::default()
            }],
            ..Default::default()
        }),
        events: Some(book_of_covers(&["prior"])),
    };

    d.dispatch(&cmd).expect("dispatch");
    let saw = saw_state.lock().unwrap().clone();
    assert_eq!(
        saw,
        Some(vec!["prior".to_string()]),
        "rejection handler must receive rebuilt prior state"
    );
}

// Multiple compensators for the same rejection ALL run, in registration
// order — distinct undoings (release funds AND notify) are independently
// registered. Their compensation events merge into one response.
#[test]
fn multiple_compensators_all_run_in_order() {
    let order = Arc::new(Mutex::new(Vec::new()));
    fn one_page_events() -> Result<pb::BusinessResponse, crate::error::HandlerError> {
        Ok(pb::BusinessResponse {
            result: Some(pb::business_response::Result::Events(pb::EventBook {
                pages: vec![pb::EventPage::default()],
                ..Default::default()
            })),
        })
    }
    let first = order.clone();
    let second = order.clone();
    let d = AggregateDispatch::new("agg-test", "payment", fresh_rebuilder())
        .on_rejected(FQ_RESERVE, move |_, _, _: &mut TestState, _| {
            first.lock().unwrap().push("first");
            one_page_events()
        })
        .on_rejected(FQ_RESERVE, move |_, _, _: &mut TestState, _| {
            second.lock().unwrap().push("second");
            one_page_events()
        });

    let resp = d
        .dispatch(&command_for(notification_command_for(FQ_RESERVE)))
        .expect("dispatch");
    assert_eq!(*order.lock().unwrap(), vec!["first", "second"]);
    assert_eq!(
        events_of(&resp).map(|b| b.pages.len()),
        Some(2),
        "merged compensation events"
    );
}

fn reject_with(
    result: Option<pb::business_response::Result>,
) -> Result<pb::BusinessResponse, crate::error::HandlerError> {
    Ok(pb::BusinessResponse { result })
}

fn events_in(domain: &str, pages: usize) -> Option<pb::business_response::Result> {
    Some(pb::business_response::Result::Events(pb::EventBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages: vec![pb::EventPage::default(); pages],
        ..Default::default()
    }))
}

/// The merged events book's cover domain and page count.
fn events_shape(resp: &pb::BusinessResponse) -> Option<(String, usize)> {
    events_of(resp).map(|b| {
        (
            b.cover
                .as_ref()
                .map(|c| c.domain.clone())
                .unwrap_or_default(),
            b.pages.len(),
        )
    })
}

fn escalate_to(domain: &str) -> Option<pb::business_response::Result> {
    Some(pb::business_response::Result::Notification(
        pb::Notification {
            cover: Some(pb::Cover {
                domain: domain.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        },
    ))
}

fn revoke(reason: &str) -> Option<pb::business_response::Result> {
    Some(pb::business_response::Result::Revocation(
        pb::RevocationResponse {
            reason: reason.to_string(),
            ..Default::default()
        },
    ))
}

/// Two compensators for FQ_RESERVE returning `first` then `second`, recording
/// their run order.
fn two_compensators(
    first: Option<pb::business_response::Result>,
    second: Option<pb::business_response::Result>,
) -> (AggregateDispatch<TestState>, Arc<Mutex<Vec<&'static str>>>) {
    let order = Arc::new(Mutex::new(Vec::new()));
    let (o1, o2) = (order.clone(), order.clone());
    let d = AggregateDispatch::new("agg-test", "payment", fresh_rebuilder())
        .on_rejected(FQ_RESERVE, move |_, _, _: &mut TestState, _| {
            o1.lock().unwrap().push("first");
            reject_with(first.clone())
        })
        .on_rejected(FQ_RESERVE, move |_, _, _: &mut TestState, _| {
            o2.lock().unwrap().push("second");
            reject_with(second.clone())
        });
    (d, order)
}

// With several compensators an escalation is never dropped: every compensator
// still runs, and the first escalation (Notification or Revocation) is the
// response — the same rule a single compensator's verbatim response follows.
#[test]
fn fan_out_escalation_after_events_wins() {
    let (d, order) = two_compensators(events_in("payment", 1), escalate_to("upstream"));
    let resp = d
        .dispatch(&command_for(notification_command_for(FQ_RESERVE)))
        .expect("dispatch");
    assert_eq!(*order.lock().unwrap(), vec!["first", "second"]);
    assert_eq!(resp.result, escalate_to("upstream"));
}

#[test]
fn fan_out_first_escalation_wins_over_a_later_one() {
    let (d, order) = two_compensators(revoke("undo"), escalate_to("upstream"));
    let resp = d
        .dispatch(&command_for(notification_command_for(FQ_RESERVE)))
        .expect("dispatch");
    assert_eq!(*order.lock().unwrap(), vec!["first", "second"]);
    assert_eq!(resp.result, revoke("undo"));
}

#[test]
fn fan_out_events_merge_under_the_first_book_cover() {
    let (d, _) = two_compensators(events_in("payment", 1), events_in("other", 2));
    let resp = d
        .dispatch(&command_for(notification_command_for(FQ_RESERVE)))
        .expect("dispatch");
    assert_eq!(events_shape(&resp), Some(("payment".to_string(), 3)));
}

#[test]
fn fan_out_events_after_an_empty_compensator_still_merge() {
    let (d, _) = two_compensators(None, events_in("payment", 2));
    let resp = d
        .dispatch(&command_for(notification_command_for(FQ_RESERVE)))
        .expect("dispatch");
    assert_eq!(events_shape(&resp), Some(("payment".to_string(), 2)));
}

#[test]
fn fan_out_of_empty_compensators_is_an_empty_response() {
    let (d, order) = two_compensators(None, None);
    let resp = d
        .dispatch(&command_for(notification_command_for(FQ_RESERVE)))
        .expect("dispatch");
    assert_eq!(*order.lock().unwrap(), vec!["first", "second"]);
    assert_eq!(resp.result, None);
}

#[test]
fn single_compensator_escalation_is_returned() {
    let d = AggregateDispatch::new("agg-test", "payment", fresh_rebuilder())
        .on_rejected(FQ_RESERVE, |_, _, _: &mut TestState, _| {
            reject_with(escalate_to("upstream"))
        });
    let resp = d
        .dispatch(&command_for(notification_command_for(FQ_RESERVE)))
        .expect("dispatch");
    assert_eq!(resp.result, escalate_to("upstream"));
}

// Compensation events append after prior history — the rejection thunk
// needs the aggregate's next_sequence to stamp them.
#[test]
fn rejection_receives_command_context() {
    let saw = Arc::new(Mutex::new(CommandContext::default()));
    let seen = saw.clone();
    let d = AggregateDispatch::new("agg-test", "payment", fresh_rebuilder()).on_rejected(
        FQ_RESERVE,
        move |_, _, _: &mut TestState, cctx| {
            *seen.lock().unwrap() = cctx;
            Ok(pb::BusinessResponse::default())
        },
    );

    let cmd = pb::ContextualCommand {
        command: Some(pb::CommandBook {
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(
                    notification_command_for(FQ_RESERVE),
                )),
                ..Default::default()
            }],
            ..Default::default()
        }),
        events: Some(pb::EventBook {
            next_sequence: 7,
            pages: vec![pb::EventPage::default()],
            ..Default::default()
        }),
    };
    d.dispatch(&cmd).expect("dispatch");
    let cctx = saw.lock().unwrap().clone();
    assert_eq!(
        cctx.next_sequence, 7,
        "compensation stamping needs next_sequence"
    );
    assert!(cctx.had_prior_events, "had_prior_events with prior history");
}

// An undeclared rejection is the framework's to handle
// (DelegateToFramework) and yields an empty response, by declaration
// rather than by accident.
#[test]
fn undeclared_rejection_delegates_to_framework() {
    let d: AggregateDispatch<TestState> =
        AggregateDispatch::new("agg-test", "order", fresh_rebuilder());
    let resp = d
        .dispatch(&command_for(notification_command_for(
            "io.angzarr.examples.v1.SomethingElse",
        )))
        .expect("undeclared rejection must not error (framework default)");
    assert!(
        resp.result.is_none(),
        "undeclared rejection yields an empty response"
    );
}

#[test]
fn corrupt_notification_coded() {
    let d: AggregateDispatch<TestState> =
        AggregateDispatch::new("agg-test", "order", fresh_rebuilder());
    let err = d
        .dispatch(&command_for(Any {
            type_url: crate::NOTIFICATION_TYPE_URL.to_string(),
            value: vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        }))
        .expect_err("corrupt Notification must error");
    assert_eq!(err.code, codes::NOTIFICATION_DECODE_FAILED);
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

// --- compensation stamping, qualified keys, undo (Compensate) --------------

fn page_seqs(resp: &pb::BusinessResponse) -> Vec<Option<u32>> {
    events_of(resp)
        .map(|b| {
            b.pages
                .iter()
                .map(
                    |p| match p.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
                        Some(pb::page_header::SequenceType::Sequence(s)) => Some(*s),
                        _ => None,
                    },
                )
                .collect()
        })
        .unwrap_or_default()
}

fn with_history(command: Any, next_sequence: u32) -> pb::ContextualCommand {
    let mut cmd = command_for(command);
    cmd.events = Some(pb::EventBook {
        next_sequence,
        pages: vec![pb::EventPage::default()],
        ..Default::default()
    });
    cmd
}

// Compensation events append after prior history (C-0083): header-less
// pages take consecutive sequences from next_sequence.
#[test]
fn compensation_events_are_stamped_after_prior_history() {
    let d = AggregateDispatch::new("agg-test", "payment", fresh_rebuilder())
        .on_rejected(FQ_RESERVE, |_, _, _: &mut TestState, _| {
            reject_with(events_in("payment", 2))
        });
    let resp = d
        .dispatch(&with_history(notification_command_for(FQ_RESERVE), 7))
        .expect("dispatch");
    assert_eq!(page_seqs(&resp), vec![Some(7), Some(8)]);
}

/// A rejection Notification for FQ_RESERVE sent to `target_domain`.
fn rejection_sent_to(target_domain: &str) -> Any {
    let mut any = notification_command_for(FQ_RESERVE);
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
    any
}

fn labelled(
    label: &'static str,
) -> impl Fn(
    &pb::Notification,
    &pb::RejectionNotification,
    &mut TestState,
    CommandContext,
) -> Result<pb::BusinessResponse, crate::error::HandlerError>
       + Send
       + Sync
       + 'static {
    move |_, _, _, _| reject_with(events_in(label, 1))
}

// C-0481: an unqualified entry matches the command type sent to any domain.
#[test]
fn unqualified_compensates_matches_any_target_domain() {
    let d = AggregateDispatch::new("agg-test", "payment", fresh_rebuilder())
        .on_rejected(FQ_RESERVE, labelled("FundsReleased"));
    let resp = d
        .dispatch(&command_for(rejection_sent_to("warehouse")))
        .expect("dispatch");
    assert_eq!(events_shape(&resp), Some(("FundsReleased".to_string(), 1)));
}

// C-0482: a domain-qualified entry matches only rejections from its domain.
#[test]
fn qualified_compensates_matches_only_its_domain() {
    let d = AggregateDispatch::new("agg-test", "payment", fresh_rebuilder())
        .on_rejected(
            &format!("inventory:{FQ_RESERVE}"),
            labelled("FundsReleased"),
        )
        .on_rejected(
            &format!("warehouse:{FQ_RESERVE}"),
            labelled("WorkflowFailed"),
        );
    let resp = d
        .dispatch(&command_for(rejection_sent_to("warehouse")))
        .expect("dispatch");
    assert_eq!(events_shape(&resp), Some(("WorkflowFailed".to_string(), 1)));
    let resp = d
        .dispatch(&command_for(rejection_sent_to("billing")))
        .expect("dispatch");
    assert_eq!(resp.result, None, "no entry matches billing");
}

#[test]
fn validate_refuses_a_type_listed_both_unqualified_and_qualified() {
    let d = AggregateDispatch::new("agg-test", "payment", fresh_rebuilder())
        .on_rejected(FQ_RESERVE, labelled("a"));
    assert!(d.validate().is_ok());
    let d = d.on_rejected(&format!("inventory:{FQ_RESERVE}"), labelled("b"));
    assert_eq!(
        d.validate().unwrap_err().code,
        codes::AMBIGUOUS_COMPENSATION
    );
}

const FQ_ADJUST: &str = "inventory.AdjustStock";
const FQ_COUNT: &str = "inventory.CountStock";

fn compensate_command(command_type: &str) -> Any {
    let notification = pb::Notification {
        payload: Some(Any {
            type_url: format!("{TYPE_URL_PREFIX}io.angzarr.v1.Compensate"),
            value: prost::Message::encode_to_vec(&pb::Compensate {
                command_type: command_type.to_string(),
                sequences: vec![4],
                reason: "aborted".to_string(),
            }),
        }),
        ..Default::default()
    };
    Any {
        type_url: format!("{TYPE_URL_PREFIX}io.angzarr.v1.Notification"),
        value: prost::Message::encode_to_vec(&notification),
    }
}

// C-0478: a Compensate routes to the undo handler for its command type, with
// rebuilt state and the Compensate, and its events append after history.
#[test]
fn compensate_routes_to_the_undo_handler_for_its_command_type() {
    let ran = Arc::new(Mutex::new(Vec::new()));
    let (adjust, reserve) = (ran.clone(), ran.clone());
    let d = AggregateDispatch::new("agg-test", "inventory", cover_applier(fresh_rebuilder()))
        .on_undo(FQ_RESERVE, move |_, _, _: &mut TestState, _| {
            reserve.lock().unwrap().push("reserve".to_string());
            reject_with(events_in("inventory", 1))
        })
        .on_undo(
            FQ_ADJUST,
            move |_, compensate, state: &mut TestState, cctx| {
                adjust.lock().unwrap().push(format!(
                    "adjust {:?} {} {}",
                    compensate.sequences,
                    state.applied.len(),
                    cctx.next_sequence
                ));
                reject_with(events_in("inventory", 1))
            },
        );
    let mut cmd = with_history(compensate_command(FQ_ADJUST), 5);
    cmd.events = Some(pb::EventBook {
        next_sequence: 5,
        ..book_of_covers(&["prior"])
    });
    let resp = d.dispatch(&cmd).expect("dispatch");
    assert_eq!(*ran.lock().unwrap(), vec!["adjust [4] 1 5".to_string()]);
    assert_eq!(page_seqs(&resp), vec![Some(5)]);
}

// C-0479: a Compensate with no undo handler is UNIMPLEMENTED, never dropped.
#[test]
fn compensate_without_an_undo_handler_is_unimplemented() {
    let d = AggregateDispatch::new("agg-test", "inventory", fresh_rebuilder())
        .on_undo(FQ_ADJUST, |_, _, _: &mut TestState, _| reject_with(None))
        .on_rejected(FQ_COUNT, labelled("not-an-undo"));
    let err = d
        .dispatch(&command_for(compensate_command(FQ_COUNT)))
        .expect_err("no undo handler");
    assert_eq!(err.code, codes::NO_UNDO_HANDLER);
    assert_eq!(err.grpc, crate::error::GrpcCode::Unimplemented);
    assert_eq!(
        err.extras
            .get(crate::error::extras::COMMAND_TYPE)
            .map(String::as_str),
        Some(FQ_COUNT)
    );
}

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

#[test]
fn claims_notification_reflects_declared_compensations_and_undos() {
    let d = AggregateDispatch::new("agg-test", "inventory", fresh_rebuilder())
        .on_rejected(&format!("warehouse:{FQ_RESERVE}"), labelled("a"))
        .on_undo(FQ_ADJUST, |_, _, _: &mut TestState, _| reject_with(None));
    assert!(d.claims_notification(&rejection_sent_to("warehouse")));
    assert!(!d.claims_notification(&rejection_sent_to("inventory")));
    assert!(d.claims_notification(&compensate_command(FQ_ADJUST)));
    assert!(!d.claims_notification(&compensate_command(FQ_COUNT)));
    assert!(!d.claims_notification(&Any {
        type_url: "/io.angzarr.v1.Notification".to_string(),
        value: vec![0xff, 0xff],
    }));
}
