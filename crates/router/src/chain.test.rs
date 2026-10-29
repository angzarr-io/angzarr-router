//! The refusal path end to end, composed in-process from the router's pure
//! calls alone, as annals' kernel composes them (plan ruling 4: the seam is
//! router dispatch). Each hop is its own step (plan ruling 2): the test
//! plays the runtime between hops, stamping identity and reading streams,
//! and commits nothing.
//!
//! The owner's words (decision 14.31): "policy calls foreign command handler,
//! which refuses. policy then calls its domain command handler and passes it a
//! fact (that the foreign CH refused)".

use prost::Message;

use crate::domain::{Admission, Arrival, DomainDispatch, FactRecord, StreamStatus};
use crate::error::{CodedError, HandlerError};
use crate::pb;
use crate::policy::{address_refusal_fact, PolicyDispatch};
use crate::test_support::*;

const INSTANCE: u8 = 7;
const EDITION: &str = "what-if";

fn wallet() -> DomainDispatch<TestState> {
    DomainDispatch::new("Wallet", "wallet", fresh_rebuilder()).on_command(
        &fq::<HoldFunds>(),
        |command, _state, _ctx| {
            let hold = HoldFunds::decode(command.value.as_slice()).expect("HoldFunds");
            Err(HandlerError::Coded(
                CodedError::rejection_precondition_failed("INSUFFICIENT_FUNDS", "holds 100", [])
                    .with_refusal(any_of(&InsufficientFunds {
                        available: 100,
                        requested: hold.amount,
                    })),
            ))
        },
    )
}

fn buyin() -> DomainDispatch<TestState> {
    DomainDispatch::process_manager("BuyIn", "buyin", fresh_rebuilder())
        .on_command(&fq::<StartBuyIn>(), |command, _state, _ctx| {
            let start = StartBuyIn::decode(command.value.as_slice()).expect("StartBuyIn");
            Ok(Some(pb::EventBook {
                pages: vec![event_page(any_of(&BuyInStarted {
                    amount: start.amount,
                }))],
                ..Default::default()
            }))
        })
        .on_refused(&fq::<HoldFunds>(), |refused, _state, _cctx| {
            let typed = InsufficientFunds::decode(
                refused.refusal.as_ref().expect("typed").value.as_slice(),
            )
            .expect("InsufficientFunds");
            Ok(FactRecord::from(any_of(&BuyInFailed {
                requested: typed.requested,
            })))
        })
        .on_fact(&fq::<FundsUnavailable>(), |fact, _state| {
            let unavailable =
                FundsUnavailable::decode(fact.value.as_slice()).expect("FundsUnavailable");
            Ok(FactRecord::from(any_of(&BuyInFailed {
                requested: unavailable.available,
            })))
        })
        .starts(&fq::<StartBuyIn>())
}

fn buyin_wallet() -> PolicyDispatch {
    PolicyDispatch::new("BuyInWallet", "buyin", "wallet").on_event(
        &fq::<BuyInStarted>(),
        |event, _cover| {
            let started = BuyInStarted::decode(event.value.as_slice()).expect("BuyInStarted");
            Ok(vec![pb::CommandBook {
                cover: Some(stream("wallet", None, 1)),
                page: Some(command_page(any_of(&HoldFunds {
                    amount: started.amount,
                }))),
                ..Default::default()
            }])
        },
    )
}

/// Runs the chain from StartBuyIn (in `edition`) to the buy-in's answer to
/// the refusal, and returns the event it records.
fn run(policy: PolicyDispatch, edition: Option<&str>) -> pb::EventPage {
    let pm = buyin();
    let instance = stream("buyin", edition, INSTANCE);

    // 1. The PM starts: a client's StartBuyIn to a new instance, in the
    //    edition (forked by the runtime) or in main.
    let start = command_book(instance.clone(), any_of(&StartBuyIn { amount: 150 }), 1);
    let new_instance = StreamStatus { exists: false };
    assert_eq!(
        pm.admit(&Arrival {
            command: &start,
            recorded: &[],
            stream: new_instance,
        })
        .expect("a starting command"),
        Admission::Dispatch
    );
    let started = pm
        .dispatch(&pb::ContextualCommand {
            command: Some(start),
            ..Default::default()
        })
        .expect("started")
        .events
        .expect("its events");

    // 2. The runtime commits BuyInStarted at sequence 0; the policy translates.
    let mut trigger = pb::EventBook {
        cover: Some(instance.clone()),
        pages: started.pages.clone(),
        ..Default::default()
    };
    trigger.pages[0].sequence = 0;
    let sent = policy.dispatch(&trigger).expect("translated").commands;
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].cover.as_ref().and_then(|c| c.edition.as_deref()),
        edition,
        "a policy's command stays in its trigger's edition"
    );
    let mut hold = caused_by(sent[0].clone(), instance.clone(), 0);
    let page = hold.page.as_mut().unwrap();
    page.command_id = Some(uuid_of(9));
    page.correlation_id = Some(uuid_of(INSTANCE));

    // 3. The wallet (forked in the edition by the runtime, which owns
    //    edition lifecycle) refuses.
    let admission = wallet()
        .admit(&Arrival {
            command: &hold,
            recorded: &[],
            stream: StreamStatus { exists: true },
        })
        .expect("admitted");
    assert_eq!(admission, Admission::Dispatch);
    let refused = wallet()
        .dispatch(&pb::ContextualCommand {
            command: Some(hold.clone()),
            ..Default::default()
        })
        .expect_err("refused");

    // 4. Back to the policy: OnRefused builds the fact; the runtime addresses it.
    let refusal = pb::Refusal {
        event: Some(trigger),
        command: Some(hold),
        response: Some(refused.refusal_response().expect("a business rejection")),
    };
    let answer = policy.dispatch_refusal(&refusal).expect("answered");
    let fact = address_refusal_fact(&refusal, &answer).expect("addressed");
    assert_eq!(fact.cover, Some(instance));

    // 5. The buy-in: its exact stream admits the fact, which records its own
    //    event.
    assert_eq!(
        pm.admit(&Arrival {
            command: &fact,
            recorded: &[],
            stream: StreamStatus { exists: true },
        })
        .expect("its own stream"),
        Admission::Dispatch
    );
    let recorded = pm
        .dispatch(&pb::ContextualCommand {
            command: Some(fact),
            events: Some(started),
            ..Default::default()
        })
        .expect("recorded")
        .events
        .expect("its events");
    recorded.pages[0].clone()
}

#[test]
fn a_refusal_passes_through_to_the_policies_source_in_process() {
    let recorded = run(buyin_wallet(), None);
    assert_eq!(
        recorded.payload,
        Some(any_of(&BuyInFailed { requested: 150 }))
    );
}

#[test]
fn a_refusal_is_translated_for_the_policies_source_in_process() {
    let policy = buyin_wallet().on_refused(&fq::<HoldFunds>(), |refusal| {
        let typed = refusal
            .response
            .as_ref()
            .and_then(|r| r.refusal.as_ref())
            .expect("typed");
        let insufficient =
            InsufficientFunds::decode(typed.value.as_slice()).expect("InsufficientFunds");
        Ok(any_of(&FundsUnavailable {
            available: insufficient.available,
        }))
    });
    let recorded = run(policy, None);
    assert_eq!(
        recorded.payload,
        Some(any_of(&BuyInFailed { requested: 100 }))
    );
}

// The same chain in an edition: the policy's command stays in its trigger's
// edition, and the refusal fact returns to the buy-in's stream in the
// edition. (Forking the wallet there is the runtime's.)
#[test]
fn the_refusal_path_stays_in_its_edition() {
    let recorded = run(buyin_wallet(), Some(EDITION));
    assert_eq!(
        recorded.payload,
        Some(any_of(&BuyInFailed { requested: 150 }))
    );
}
