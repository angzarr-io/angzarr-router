//! The refusal path end to end, composed in-process from the router's pure
//! calls alone, as annals' kernel composes them (plan ruling 4: the seam is
//! router dispatch). Each hop is its own step (plan ruling 2): the test
//! plays the runtime between hops, admitting inputs (decision 14.42),
//! stamping identity and reading streams, and commits nothing.
//!
//! The owner's words (decision 14.31): "policy calls foreign command handler,
//! which refuses. policy then calls its domain command handler and passes it a
//! fact (that the foreign CH refused)".

use prost::Message;

use crate::domain::{CommandContext, DomainDispatch, FactRecord};
use crate::error::{CodedError, HandlerError};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::test_support::*;

const INSTANCE: u8 = 7;
const EDITION: &str = "what-if";

fn wallet() -> DomainDispatch<TestState> {
    DomainDispatch::new("Wallet", "wallet", fresh_rebuilder()).on_command(
        &fq::<HoldFunds>(),
        |command, _state, _ctx| {
            let hold = HoldFunds::decode(command.value.as_slice()).expect("HoldFunds");
            Err(HandlerError::Coded(CodedError::refused(any_of(
                &InsufficientFunds {
                    available: 100,
                    requested: hold.amount,
                },
            ))))
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

/// The refused policy's view of a business refusal, as the runtime builds
/// it (types.proto, RefusalResponse; decision 14.42): its typed refusal
/// object, else its key and args.
fn runtime_refusal_response(refused: &CodedError) -> pb::RefusalResponse {
    pb::RefusalResponse {
        reason: Some(match refused.refusal.as_deref() {
            Some(typed) => pb::refusal_response::Reason::Refusal(typed.clone()),
            None => pb::refusal_response::Reason::Diagnostic(refused.diagnostic()),
        }),
    }
}

/// The refusal fact the runtime builds from OnRefused's `answer` (decision
/// 14.42): the translated fact, or with none the standard pass-through
/// `CommandRefused`, addressed to the trigger's exact stream under the
/// refused command's identity.
fn runtime_refusal_fact(refusal: &pb::Refusal, answer: &pb::OnRefusedResponse) -> pb::CommandBook {
    let refused = refusal.command.clone().expect("the refused command");
    let trigger = refusal.event.as_ref().expect("the trigger");
    let body = match answer.fact.as_ref().and_then(|f| f.page.as_ref()) {
        Some(page) => page.body.clone(),
        None => Some(pb::command_page::Body::Fact(pb::Fact {
            payload: Some(any_of(&pb::CommandRefused {
                command: Some(refused.clone()),
                refusal: refusal.response.as_ref().and_then(|r| match &r.reason {
                    Some(pb::refusal_response::Reason::Refusal(typed)) => Some(typed.clone()),
                    _ => None,
                }),
                trigger: Some(pb::EventRef {
                    stream: trigger.cover.clone(),
                    sequence: trigger.pages.last().map_or(0, |p| p.sequence),
                }),
            })),
        })),
    };
    let page = refused.page.expect("its page");
    let command_id = page.command_id.expect("its id");
    pb::CommandBook {
        cover: trigger.cover.clone(),
        page: Some(pb::CommandPage {
            body,
            fact_origin: Some(pb::FactOrigin {
                external_id: command_id.to_string(),
                ..Default::default()
            }),
            command_id: Some(command_id),
            correlation_id: page.correlation_id,
            causation_id: page.causation_id,
        }),
        handling: None,
    }
}

/// Runs the chain from StartBuyIn (in `edition`) to the buy-in's answer to
/// the refusal, and returns the event it records.
fn run(policy: PolicyDispatch, edition: Option<&str>) -> pb::EventPage {
    let pm = buyin();
    let instance = stream("buyin", edition, INSTANCE);

    // 1. The PM starts: a client's StartBuyIn to a new instance, in the
    //    edition (forked by the runtime) or in main.
    let start = command_book(instance.clone(), any_of(&StartBuyIn { amount: 150 }), 1);
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
    let sent = commands_of(&policy.dispatch(&trigger).expect("translated"));
    assert_eq!(sent.len(), 1);
    // The runtime checks the command and stamps its trigger's edition
    // (decision 14.42): a policy's command stays in that edition.
    let mut hold = sent[0].clone();
    hold.cover.as_mut().unwrap().edition = edition.map(str::to_string);
    let mut hold = caused_by(hold, instance.clone(), 0);
    let page = hold.page.as_mut().unwrap();
    page.command_id = Some(uuid_of(9));
    page.correlation_id = Some(uuid_of(INSTANCE));

    // 3. The wallet (forked in the edition by the runtime, which owns
    //    edition lifecycle) refuses.
    let refused = wallet()
        .dispatch(&pb::ContextualCommand {
            command: Some(hold.clone()),
            ..Default::default()
        })
        .expect_err("refused");

    // 4. Back to the policy: OnRefused translates, or answers no translation;
    //    the runtime builds the pass-through fact and addresses it.
    let refusal = pb::Refusal {
        event: Some(trigger),
        command: Some(hold),
        response: Some(runtime_refusal_response(&refused)),
    };
    let answer = policy
        .dispatch_refusal(&refusal, &CommandContext::default())
        .expect("answered");
    let fact = runtime_refusal_fact(&refusal, &answer);
    assert_eq!(fact.cover, Some(instance));

    // 5. The buy-in records its own event for the fact.
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
    let policy = buyin_wallet().on_refused(&fq::<HoldFunds>(), |refusal, _ctx| {
        let typed = refusal
            .response
            .as_ref()
            .and_then(|r| match &r.reason {
                Some(pb::refusal_response::Reason::Refusal(typed)) => Some(typed),
                _ => None,
            })
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
// edition (the runtime stamps it), and the refusal fact returns to the
// buy-in's stream in the edition. (Forking the wallet there is the
// runtime's.)
#[test]
fn the_refusal_path_stays_in_its_edition() {
    let recorded = run(buyin_wallet(), Some(EDITION));
    assert_eq!(
        recorded.payload,
        Some(any_of(&BuyInFailed { requested: 150 }))
    );
}
