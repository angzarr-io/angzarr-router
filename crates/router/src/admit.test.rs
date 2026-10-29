//! A domain's input on the current spec (angzarr-project spec/20261019-draft
//! at 636b313): typed refusals, the process manager as a domain, and the
//! rules an input meets before its handler runs ([`DomainDispatch::admit`]).
//!
//! Spec governing each group:
//! - typed refusals: options.proto CommandOptions.refuses; types.proto
//!   RefusalResponse; decision 14.31 (Refusals, "A typed refusal object").
//! - a process manager is a domain: process_manager.proto; decision 14.31.
//! - the start rule: process_manager.proto, Starting an instance;
//!   options.proto CommandOptions.starts; decision 14.31 (Start rule).
//! - deduplication: types.proto, the command_identity region
//!   (Deduplication); decision 14.31 (Identity).
//! - the refusal fact's exact stream: types.proto, the command_identity
//!   region (Validation); decision 14.31 ("The exact stream").
//!
//! annals calls these in-process (plan ruling 4: the seam is router
//! dispatch), core's coordinator over the wire; neither reimplements them.

use prost::Message;
use prost_types::Any;

use crate::domain::{Admission, Arrival, DomainDispatch, FactRecord, StreamStatus};
use crate::error::{codes, CodedError, GrpcCode, HandlerError};
use crate::pb;
use crate::test_support::*;
use crate::upcaster::UpcasterDispatch;

// --- fixtures ----------------------------------------------------------------

/// The wallet aggregate: HoldFunds holds, or is refused with the typed
/// refusal InsufficientFunds when it asks for more than 100.
fn wallet() -> DomainDispatch<TestState> {
    DomainDispatch::new("Wallet", "wallet", fresh_rebuilder()).on_command(
        &fq::<HoldFunds>(),
        |command, _state, _ctx| {
            let hold = HoldFunds::decode(command.value.as_slice()).expect("HoldFunds");
            if hold.amount > 100 {
                return Err(HandlerError::Coded(CodedError::refused(any_of(
                    &InsufficientFunds {
                        available: 100,
                        requested: hold.amount,
                    },
                ))));
            }
            Ok(Some(pb::EventBook {
                pages: vec![event_page(cover_any("held"))],
                ..Default::default()
            }))
        },
    )
}

/// The buy-in process manager: StartBuyIn (starting) records BuyInStarted;
/// RecordFundsHeld (not starting) records nothing new; BuyInTimedOut is a
/// fact (not starting); a refused HoldFunds, passed through, records
/// BuyInFailed with the amount the wallet was asked for.
fn buyin() -> DomainDispatch<TestState> {
    DomainDispatch::process_manager("BuyIn", "buyin", cover_applier(fresh_rebuilder()))
        .on_command(&fq::<StartBuyIn>(), |command, _state, _ctx| {
            let start = StartBuyIn::decode(command.value.as_slice()).expect("StartBuyIn");
            Ok(Some(pb::EventBook {
                pages: vec![event_page(any_of(&BuyInStarted {
                    amount: start.amount,
                }))],
                ..Default::default()
            }))
        })
        .on_command(&fq::<RecordFundsHeld>(), |_command, _state, _ctx| Ok(None))
        .on_fact(&fq::<BuyInTimedOut>(), |fact, _state| {
            Ok(FactRecord::from(fact.clone()))
        })
        .on_refused(&fq::<HoldFunds>(), |refused, _state, _cctx| {
            let held = refused
                .command
                .as_ref()
                .and_then(|book| crate::command_payload(book.page.as_ref()?))
                .map(|any| HoldFunds::decode(any.value.as_slice()).expect("HoldFunds"))
                .expect("the refused command");
            let refusal = InsufficientFunds::decode(
                refused.refusal.as_ref().expect("typed").value.as_slice(),
            )
            .expect("InsufficientFunds");
            assert_eq!(refusal.requested, held.amount);
            Ok(FactRecord::from(any_of(&BuyInFailed {
                requested: held.amount,
            })))
        })
        .starts(&fq::<StartBuyIn>())
        .with_rules_version("1")
}

/// The PM instance's root: the one its inputs' covers name (decision 14.36).
const INSTANCE: u8 = 7;

fn arrival<'a>(command: &'a pb::CommandBook, stream: StreamStatus) -> Arrival<'a> {
    Arrival {
        command,
        recorded: &[],
        stream,
    }
}

fn existing() -> StreamStatus {
    StreamStatus { exists: true }
}

fn absent() -> StreamStatus {
    StreamStatus::default()
}

fn refused(err: CodedError, code: &str, grpc: GrpcCode) {
    assert_eq!(err.code, code);
    assert_eq!(err.grpc, grpc);
}

// --- typed refusals ------------------------------------------------------------

// A command handler refuses with a business rejection carrying its typed
// refusal object, packed; it records nothing.
#[test]
fn a_refusing_handler_answers_failed_precondition_with_its_typed_refusal() {
    let req = pb::ContextualCommand {
        command: Some(command_book(
            stream("wallet", None, 1),
            any_of(&HoldFunds { amount: 150 }),
            9,
        )),
        ..Default::default()
    };
    let err = wallet().dispatch(&req).expect_err("refused");
    assert_eq!(err.grpc, GrpcCode::FailedPrecondition);
    assert_eq!(
        err.refusal.as_deref(),
        Some(&any_of(&InsufficientFunds {
            available: 100,
            requested: 150
        }))
    );
}

// --- a process manager is a domain ----------------------------------------------

// Its inputs are commands; its only output is its own events, with the
// post-command snapshot, as an aggregate's (CommandHandlerResponse).
#[test]
fn a_process_manager_handles_a_command_and_records_its_own_events() {
    let req = pb::ContextualCommand {
        command: Some(command_book(
            stream("buyin", None, INSTANCE),
            any_of(&StartBuyIn { amount: 50 }),
            1,
        )),
        ..Default::default()
    };
    let resp = buyin().dispatch(&req).expect("handled");
    let events = resp.events.expect("its events");
    assert_eq!(
        events
            .pages
            .iter()
            .map(|p| p.payload.clone())
            .collect::<Vec<_>>(),
        vec![Some(any_of(&BuyInStarted { amount: 50 }))]
    );
    assert_eq!(resp.rules_version, "1");
}

// ... and facts (timeouts, webhooks), which it never refuses.
#[test]
fn a_process_manager_records_a_fact() {
    let req = pb::ContextualCommand {
        command: Some(fact_book(
            stream("buyin", None, INSTANCE),
            any_of(&BuyInTimedOut {}),
            "timer-1",
        )),
        events: Some(book_of_covers(&["started"])),
        ..Default::default()
    };
    let resp = buyin().dispatch(&req).expect("recorded");
    assert_eq!(
        resp.events.expect("its events").pages[0].payload,
        Some(any_of(&BuyInTimedOut {}))
    );
}

// --- the start rule (process managers only) ---------------------------------------

#[test]
fn a_starting_command_starts_an_instance() {
    let command = command_book(
        stream("buyin", None, INSTANCE),
        any_of(&StartBuyIn { amount: 5 }),
        1,
    );
    assert_eq!(
        buyin()
            .admit(&arrival(&command, absent()))
            .expect("admitted"),
        Admission::Dispatch
    );
}

#[test]
fn a_command_that_does_not_start_one_is_refused_pm_no_instance() {
    let command = command_book(
        stream("buyin", None, INSTANCE),
        any_of(&RecordFundsHeld { amount: 5 }),
        1,
    );
    refused(
        buyin()
            .admit(&arrival(&command, absent()))
            .expect_err("no instance"),
        codes::PM_NO_INSTANCE,
        GrpcCode::FailedPrecondition,
    );
}

#[test]
fn a_fact_that_does_not_start_one_is_skipped_silently() {
    let fact = fact_book(
        stream("buyin", None, INSTANCE),
        any_of(&BuyInTimedOut {}),
        "timer-1",
    );
    assert_eq!(
        buyin().admit(&arrival(&fact, absent())).expect("skipped"),
        Admission::Skip
    );
}

#[test]
fn a_starting_fact_starts_an_instance() {
    let pm = buyin().starts(&fq::<BuyInTimedOut>());
    let fact = fact_book(
        stream("buyin", None, INSTANCE),
        any_of(&BuyInTimedOut {}),
        "timer-1",
    );
    assert_eq!(
        pm.admit(&arrival(&fact, absent())).expect("admitted"),
        Admission::Dispatch
    );
}

#[test]
fn any_input_reaches_an_existing_instance() {
    let command = command_book(
        stream("buyin", None, INSTANCE),
        any_of(&RecordFundsHeld { amount: 5 }),
        1,
    );
    let fact = fact_book(
        stream("buyin", None, INSTANCE),
        any_of(&BuyInTimedOut {}),
        "timer-1",
    );
    for input in [&command, &fact] {
        assert_eq!(
            buyin()
                .admit(&arrival(input, existing()))
                .expect("admitted"),
            Admission::Dispatch
        );
    }
}

// The match is on the type URL after upcasting (decision 14.25).
#[test]
fn the_start_rule_matches_the_type_after_upcasting() {
    let pm = buyin().with_command_upcaster(
        UpcasterDispatch::new("buyin-v0", "buyin").on_event("test.buyin.StartBuyInV0", |_| {
            Ok(any_of(&StartBuyIn { amount: 1 }))
        }),
    );
    let old = command_book(
        stream("buyin", None, INSTANCE),
        Any {
            type_url: crate::type_url("test.buyin.StartBuyInV0"),
            value: Vec::new(),
        },
        1,
    );
    assert_eq!(
        pm.admit(&arrival(&old, absent())).expect("admitted"),
        Admission::Dispatch
    );
}

// An aggregate has no start rule: any command reaches a new stream.
#[test]
fn an_aggregate_has_no_start_rule() {
    let command = command_book(
        stream("wallet", None, 1),
        any_of(&HoldFunds { amount: 5 }),
        1,
    );
    assert_eq!(
        wallet()
            .admit(&arrival(&command, absent()))
            .expect("admitted"),
        Admission::Dispatch
    );
}

// A PM instance's root is the one its input's cover names, not its
// correlation (decision 14.36, withdrawing 14.30's rule): an input whose set correlation_id differs from
// its cover's root is admitted to the root its cover names, keeping its
// correlation. PM_CORRELATION_NOT_ROOT is gone; the start rule is the only
// rule a PM adds to an aggregate's.
#[test]
fn an_input_to_a_process_manager_with_another_correlation_is_admitted() {
    let mut start = command_book(
        stream("buyin", None, INSTANCE),
        any_of(&StartBuyIn { amount: 5 }),
        1,
    );
    start.page.as_mut().unwrap().correlation_id = Some(uuid_of(INSTANCE + 1));
    assert_eq!(
        buyin()
            .admit(&arrival(&start, absent()))
            .expect("a starting command with another correlation starts the instance"),
        Admission::Dispatch
    );
    assert_eq!(
        start.page.as_ref().unwrap().correlation_id,
        Some(uuid_of(INSTANCE + 1)),
        "admission keeps the input's correlation"
    );
}

// C-0659's router half: one instance, named by a root, takes the inputs of
// several correlations; a non-starting command of another correlation
// reaches the existing instance, and so does a fact.
#[test]
fn an_existing_instance_takes_inputs_of_several_correlations() {
    let mut command = command_book(
        stream("buyin", None, INSTANCE),
        any_of(&RecordFundsHeld { amount: 5 }),
        2,
    );
    command.page.as_mut().unwrap().correlation_id = Some(uuid_of(INSTANCE + 2));
    assert_eq!(
        buyin()
            .admit(&arrival(&command, existing()))
            .expect("another correlation's command reaches the instance"),
        Admission::Dispatch
    );
    let mut fact = fact_book(
        stream("buyin", None, INSTANCE),
        any_of(&BuyInTimedOut::default()),
        "timer-1",
    );
    fact.page.as_mut().unwrap().correlation_id = Some(uuid_of(INSTANCE + 3));
    assert_eq!(
        buyin()
            .admit(&arrival(&fact, existing()))
            .expect("another correlation's fact reaches the instance"),
        Admission::Dispatch
    );
}

// The start rule stays (decision 14.36): another correlation does not let a
// non-starting command start an instance.
#[test]
fn another_correlation_does_not_bypass_the_start_rule() {
    let mut command = command_book(
        stream("buyin", None, INSTANCE),
        any_of(&RecordFundsHeld { amount: 5 }),
        3,
    );
    command.page.as_mut().unwrap().correlation_id = Some(uuid_of(INSTANCE + 1));
    refused(
        buyin()
            .admit(&arrival(&command, absent()))
            .expect_err("no instance, not starting"),
        codes::PM_NO_INSTANCE,
        GrpcCode::FailedPrecondition,
    );
}

// --- deduplication by command id and fact id ----------------------------------------

#[test]
fn a_command_already_recorded_under_its_id_is_a_duplicate() {
    let command = command_book(
        stream("wallet", None, 1),
        any_of(&HoldFunds { amount: 5 }),
        9,
    );
    let recorded = vec![recorded_page(2, cover_any("held"), 9)];
    let admission = wallet()
        .admit(&Arrival {
            command: &command,
            recorded: &recorded,
            stream: existing(),
        })
        .expect("admitted");
    assert_eq!(admission, Admission::Duplicate(recorded));
}

// A process manager deduplicates like any domain, before its start rule:
// a redelivered starting command is a duplicate, never a second start.
#[test]
fn a_fact_already_recorded_under_its_id_is_a_duplicate() {
    let fact = fact_book(
        stream("buyin", None, INSTANCE),
        any_of(&BuyInTimedOut {}),
        "timer-1",
    );
    let recorded = vec![recorded_page(4, any_of(&BuyInTimedOut {}), 3)];
    assert_eq!(
        buyin()
            .admit(&Arrival {
                command: &fact,
                recorded: &recorded,
                stream: existing(),
            })
            .expect("admitted"),
        Admission::Duplicate(recorded)
    );
}

#[test]
fn a_commands_id_is_its_command_id() {
    let command = command_book(
        stream("wallet", None, 1),
        any_of(&HoldFunds { amount: 5 }),
        9,
    );
    assert_eq!(crate::input_id(&command), Some(uuid_of(9)));
}

// A fact's id is derived from its external id when the sender left it unset.
#[test]
fn a_facts_id_derives_from_its_external_id() {
    let fact = fact_book(
        stream("buyin", None, INSTANCE),
        any_of(&BuyInTimedOut {}),
        "timer-1",
    );
    assert_eq!(
        crate::input_id(&fact),
        Some(angzarr_proto::ids::fact_command_id("timer-1"))
    );
}

// --- refusal facts at their source --------------------------------------------------

/// The refusal fact a policy (buyin -> wallet) delivers to the buy-in when
/// the wallet refuses its HoldFunds, addressed to `to`; its trigger is
/// BuyInStarted at sequence 0 of `trigger`.
fn refusal_fact(to: pb::Cover, trigger: pb::Cover) -> pb::CommandBook {
    let refused_command = caused_by(
        command_book(
            stream("wallet", None, 1),
            any_of(&HoldFunds { amount: 150 }),
            9,
        ),
        trigger.clone(),
        0,
    );
    let mut fact = fact_book(
        to,
        any_of(&pb::CommandRefused {
            command: Some(refused_command),
            refusal: Some(any_of(&InsufficientFunds {
                available: 100,
                requested: 150,
            })),
            trigger: Some(pb::EventRef {
                stream: Some(trigger.clone()),
                sequence: 0,
            }),
        }),
        "00000000-0000-0000-0000-000000000000",
    );
    let page = fact.page.as_mut().unwrap();
    page.command_id = Some(uuid_of(9));
    page.causation_id = Some(pb::EventRef {
        stream: Some(trigger),
        sequence: 0,
    });
    fact
}

#[test]
fn a_refusal_fact_is_admitted_by_the_exact_stream_its_trigger_names() {
    let trigger = stream("buyin", None, INSTANCE);
    let fact = refusal_fact(trigger.clone(), trigger);
    assert_eq!(
        buyin()
            .admit(&arrival(&fact, existing()))
            .expect("admitted"),
        Admission::Dispatch
    );
}

#[test]
fn a_refusal_fact_for_any_other_stream_is_refused() {
    let trigger = stream("buyin", None, INSTANCE);
    for other in [
        stream("buyin", None, INSTANCE + 1),
        stream("buyin", Some("what-if"), INSTANCE),
    ] {
        refused(
            buyin()
                .admit(&arrival(&refusal_fact(other, trigger.clone()), existing()))
                .expect_err("another stream"),
            codes::REFUSAL_FACT_WRONG_STREAM,
            GrpcCode::InvalidArgument,
        );
    }
}

// The pass-through CommandRefused reaches the handler for the refused
// command's type, which unpacks the typed refusal and records its own events.
#[test]
fn a_passed_through_refusal_reaches_the_handler_for_the_refused_commands_type() {
    let trigger = stream("buyin", None, INSTANCE);
    let req = pb::ContextualCommand {
        command: Some(refusal_fact(trigger.clone(), trigger)),
        events: Some(book_of_covers(&["started"])),
        ..Default::default()
    };
    let resp = buyin().dispatch(&req).expect("recorded");
    assert_eq!(
        resp.events.expect("its events").pages[0].payload,
        Some(any_of(&BuyInFailed { requested: 150 }))
    );
}

// The handler for a passed-through refusal reads the refusal fact's
// context, as any fact handler does: its command id (the refused
// command's), its cover (the trigger's stream) and whether that stream has
// history.
#[test]
fn a_passed_through_refusals_handler_reads_its_context() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    let buyin = DomainDispatch::process_manager("BuyIn", "buyin", cover_applier(fresh_rebuilder()))
        .on_refused(&fq::<HoldFunds>(), move |_refused, _state, cctx| {
            sink.lock().unwrap().push(cctx);
            Ok(FactRecord::from(any_of(&BuyInFailed { requested: 150 })))
        })
        .starts(&fq::<StartBuyIn>())
        .with_rules_version("1");
    let trigger = stream("buyin", None, INSTANCE);
    let req = pb::ContextualCommand {
        command: Some(refusal_fact(trigger.clone(), trigger.clone())),
        events: Some(book_of_covers(&["started"])),
        ..Default::default()
    };
    buyin.dispatch(&req).expect("recorded");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "the handler ran once");
    assert_eq!(
        seen[0].command_id,
        Some(uuid_of(9)),
        "the refused command's id"
    );
    assert_eq!(seen[0].cover, Some(trigger), "the trigger's stream");
    assert!(seen[0].had_prior_events, "the stream has history");
}

// A refusal with no path is caught at run time (decision 14.31): NO_FACT_HANDLER.
#[test]
fn a_passed_through_refusal_of_a_command_with_no_handler_is_no_fact_handler() {
    let trigger = stream("buyin", None, INSTANCE);
    let mut fact = refusal_fact(trigger.clone(), trigger);
    let page = fact.page.as_mut().unwrap();
    let Some(pb::command_page::Body::Fact(body)) = page.body.as_mut() else {
        unreachable!()
    };
    let mut refused =
        pb::CommandRefused::decode(body.payload.as_ref().unwrap().value.as_slice()).unwrap();
    refused.command = Some(command_book(
        stream("wallet", None, 1),
        any_of(&ReleaseFunds { amount: 1 }),
        9,
    ));
    body.payload = Some(any_of(&refused));
    let err = buyin()
        .dispatch(&pb::ContextualCommand {
            command: Some(fact),
            ..Default::default()
        })
        .expect_err("no handler");
    assert_eq!(err.code, codes::NO_FACT_HANDLER);
}

// --- no edition-lifecycle outcome (owner ruling, 2026-10-28) -----------------

// The router never sees a meta-event: creating, forking, refusing an
// arrival in an unforked or deleted edition, and deleting are the runtime's,
// decided before it calls the router. So admit answers Dispatch, Duplicate
// or Skip (or a refusal), and the runtime tells it only whether the stream
// exists. Both matches are exhaustive with no wildcard: an edition outcome
// or input added back fails to compile here.
#[test]
fn admit_has_no_edition_lifecycle_outcome() {
    fn outcome(admission: &Admission) -> &'static str {
        match admission {
            Admission::Dispatch => "dispatch",
            Admission::Duplicate(_) => "duplicate",
            Admission::Skip => "skip",
        }
    }
    let StreamStatus { exists: _ } = StreamStatus::default();

    // An input in an edition is admitted exactly as one in main: the router
    // neither forks nor refuses it for its edition.
    for edition in [None, Some("what-if")] {
        let command = command_book(
            stream("wallet", edition, 1),
            any_of(&HoldFunds { amount: 5 }),
            9,
        );
        let admitted = wallet()
            .admit(&arrival(&command, absent()))
            .expect("admitted");
        assert_eq!(outcome(&admitted), "dispatch", "{edition:?}");
    }
}
