//! A policy on the current spec: a refusal of one of its commands comes back
//! through OnRefused, which answers the translating callback's fact payload
//! alone, or no translation (policy.proto, PolicyService, Refusals; decisions
//! 14.31, 14.42). Building the pass-through `CommandRefused`, and
//! addressing and identifying every refusal fact, are the coordinator's.

use prost::Message;
use prost_types::Any;

use crate::domain::CommandContext;
use crate::error::{GrpcCode, HandlerError, RouterReason};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::test_support::*;

const INSTANCE: u8 = 7;

/// The policy (buyin -> wallet): BuyInStarted becomes HoldFunds for the
/// player's wallet (root 1), addressed by the business code without an
/// edition.
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

/// The triggering event: BuyInStarted at sequence 4 of the buy-in instance
/// in `edition`.
fn trigger_book(edition: Option<&str>) -> pb::EventBook {
    pb::EventBook {
        cover: Some(stream("buyin", edition, INSTANCE)),
        pages: vec![sequenced_event_page(
            4,
            any_of(&BuyInStarted { amount: 150 }),
        )],
        ..Default::default()
    }
}

/// The HoldFunds the policy sent, as delivered: identity stamped.
fn sent_command(edition: Option<&str>) -> pb::CommandBook {
    let mut book = caused_by(
        command_book(
            stream("wallet", edition, 1),
            any_of(&HoldFunds { amount: 150 }),
            9,
        ),
        stream("buyin", edition, INSTANCE),
        4,
    );
    book.page.as_mut().unwrap().correlation_id = Some(uuid_of(INSTANCE));
    book
}

fn insufficient() -> Any {
    any_of(&InsufficientFunds {
        available: 100,
        requested: 150,
    })
}

fn refusal(edition: Option<&str>, typed: Option<Any>) -> pb::Refusal {
    pb::Refusal {
        event: Some(trigger_book(edition)),
        command: Some(sent_command(edition)),
        response: Some(pb::RefusalResponse {
            reason: Some(match typed {
                Some(typed) => pb::refusal_response::Reason::Refusal(typed),
                None => pb::refusal_response::Reason::Diagnostic(pb::Diagnostic {
                    code: "WALLET_EMPTY".to_string(),
                    ..Default::default()
                }),
            }),
        }),
    }
}

/// The fact of an OnRefused answer, decoded as `M`.
fn fact_of<M: Message + Default>(answer: &pb::OnRefusedResponse) -> M {
    let book = answer.fact.as_ref().expect("a fact");
    let any = crate::fact_payload(book.page.as_ref().expect("its page")).expect("a Fact arm");
    M::decode(any.value.as_slice()).expect("the fact's type")
}

// --- a policy translates an event into a command --------------------------------------

#[test]
fn a_policy_translates_an_event_into_a_command_for_its_destination() {
    let resp = buyin_wallet()
        .dispatch(&trigger_book(None))
        .expect("translated");
    assert_eq!(commands_of(&resp).len(), 1);
    assert_eq!(
        crate::command_payload(commands_of(&resp)[0].page.as_ref().unwrap()),
        Some(&any_of(&HoldFunds { amount: 150 }))
    );
}

// --- no translation (C-0692) ------------------------------------------------------------

#[test]
fn with_no_callback_the_answer_carries_no_fact() {
    for typed in [Some(insufficient()), None] {
        let answer = buyin_wallet()
            .dispatch_refusal(&refusal(None, typed), &CommandContext::default())
            .expect("answered");
        assert_eq!(answer, pb::OnRefusedResponse { fact: None });
    }
}

// --- translate --------------------------------------------------------------------------

#[test]
fn translate_answers_the_callbacks_domestic_fact() {
    let policy = buyin_wallet().on_refused(&fq::<HoldFunds>(), |refusal, _ctx| {
        let typed = refusal
            .response
            .as_ref()
            .and_then(|r| match &r.reason {
                Some(pb::refusal_response::Reason::Refusal(typed)) => Some(typed),
                _ => None,
            })
            .expect("the typed refusal");
        let insufficient =
            InsufficientFunds::decode(typed.value.as_slice()).expect("InsufficientFunds");
        Ok(any_of(&FundsUnavailable {
            available: insufficient.available,
        }))
    });
    let answer = policy
        .dispatch_refusal(
            &refusal(None, Some(insufficient())),
            &CommandContext::default(),
        )
        .expect("answered");
    let fact: FundsUnavailable = fact_of(&answer);
    assert_eq!(fact, FundsUnavailable { available: 100 });
    // C-0691: the payload alone; its cover, handling and identity are the
    // coordinator's.
    assert_eq!(
        answer.fact,
        Some(pb::CommandBook {
            page: Some(fact_page(any_of(&FundsUnavailable { available: 100 }))),
            ..Default::default()
        })
    );
}

// The translating callback reads the refused command's context, whole, as
// the runtime passes it on from the target's dispatch: its cover, its
// sequences, history, resolution and conflict, and its identity.
#[test]
fn a_translating_callback_reads_the_refused_commands_context() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    let policy = buyin_wallet().on_refused(&fq::<HoldFunds>(), move |_refusal, ctx| {
        sink.lock().unwrap().push(ctx.clone());
        Ok(any_of(&FundsUnavailable { available: 0 }))
    });
    let window = vec![sequenced_event_page(
        3,
        any_of(&FundsUnavailable { available: 1 }),
    )];
    let ctx = CommandContext {
        next_sequence: 4,
        had_prior_events: true,
        cover: Some(stream("wallet", None, 1)),
        expected_sequence: 3,
        conflict_resolution: pb::ConflictResolution::AggregateHandles,
        conflict: Some(crate::domain::Conflict::new(window.clone(), vec![7, 7])),
        command_id: Some(uuid_of(9)),
        correlation_id: Some(uuid_of(8)),
        fact_origin: None,
        recorded_at: None,
    };
    policy
        .dispatch_refusal(&refusal(None, Some(insufficient())), &ctx)
        .expect("answered");
    let seen = seen.lock().unwrap();
    let [got] = seen.as_slice() else {
        panic!("the callback ran once");
    };
    assert_eq!(
        (
            got.next_sequence,
            got.had_prior_events,
            got.expected_sequence
        ),
        (4, true, 3)
    );
    assert_eq!(got.cover, Some(stream("wallet", None, 1)));
    assert_eq!(
        got.conflict_resolution,
        pb::ConflictResolution::AggregateHandles
    );
    let conflict = got.conflict.as_ref().expect("the conflict");
    assert_eq!(
        (conflict.window(), conflict.nonce()),
        (window.as_slice(), &[7u8, 7][..])
    );
    assert_eq!(
        (got.command_id.clone(), got.correlation_id.clone()),
        (Some(uuid_of(9)), Some(uuid_of(8)))
    );
}

// The callback's presence is translate mode for THAT command type only.
#[test]
fn a_command_type_with_no_callback_has_no_translation() {
    let policy = buyin_wallet().on_refused(&fq::<ReleaseFunds>(), |_, _| {
        panic!("not the refused command's type")
    });
    let answer = policy
        .dispatch_refusal(
            &refusal(None, Some(insufficient())),
            &CommandContext::default(),
        )
        .expect("answered");
    assert_eq!(answer.fact, None);
}

// A failing callback is a policy bug: dead-lettered and alerted by the runtime.
#[test]
fn a_failing_callback_is_a_policy_bug() {
    let policy = buyin_wallet().on_refused(&fq::<HoldFunds>(), |_, _| {
        Err(HandlerError::Other("cannot translate".to_string()))
    });
    let err = policy
        .dispatch_refusal(
            &refusal(None, Some(insufficient())),
            &CommandContext::default(),
        )
        .expect_err("a bug");
    assert_eq!(err.code, RouterReason::UnhandledHandlerError.as_str_name());
    assert_eq!(err.grpc, GrpcCode::Internal);
}

#[test]
fn a_policy_router_answers_a_refusal_and_no_other_router_does() {
    let refusal = refusal(None, Some(insufficient()));
    let router = crate::router::RouterBuilder::new()
        .policy(buyin_wallet().on_refused(&fq::<HoldFunds>(), |_, _| {
            Ok(any_of(&FundsUnavailable { available: 2 }))
        }))
        .build()
        .expect("build");
    let answer = router
        .dispatch_refusal(&refusal, &CommandContext::default())
        .expect("answered");
    let fact: FundsUnavailable = fact_of(&answer);
    assert_eq!(fact, FundsUnavailable { available: 2 });

    let projector = crate::router::RouterBuilder::new()
        .projector(crate::projector::ProjectorDispatch::new("p", || ()))
        .build()
        .expect("build");
    let err = projector
        .dispatch_refusal(&refusal, &CommandContext::default())
        .expect_err("no policy");
    assert_eq!(err.code, "ROUTER_REASON_NO_COMPONENT_OF_KIND");
}
