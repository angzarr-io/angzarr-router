//! A saga on the current spec: its commands stay in its trigger's edition,
//! and a refusal of one of them comes back through OnRefused as the one
//! refusal fact for its own source domain, passed through or translated
//! (saga.proto, SagaService, Refusals; types.proto, the refusal_facts and
//! command_identity regions; decision 14.31).

use prost::Message;
use prost_types::Any;

use crate::error::{codes, CodedError, GrpcCode, HandlerError};
use crate::pb;
use crate::saga::{address_refusal_fact, SagaDispatch};
use crate::test_support::*;

const INSTANCE: u8 = 7;

/// The saga (buyin -> wallet): BuyInStarted becomes HoldFunds for the
/// player's wallet (root 1), addressed by the business code without an
/// edition.
fn buyin_wallet() -> SagaDispatch {
    SagaDispatch::new("BuyInWallet", "buyin", "wallet").on_event(
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

fn trigger_ref(edition: Option<&str>) -> pb::EventRef {
    pb::EventRef {
        stream: Some(stream("buyin", edition, INSTANCE)),
        sequence: 4,
    }
}

/// The HoldFunds the saga sent, as delivered: identity stamped.
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
            refusal: typed,
            message: "the wallet holds 100".to_string(),
        }),
    }
}

/// The fact of an OnRefused answer, decoded as `M`.
fn fact_of<M: Message + Default>(answer: &pb::OnRefusedResponse) -> M {
    let book = answer.fact.as_ref().expect("a fact");
    let any = crate::fact_payload(book.page.as_ref().expect("its page")).expect("a Fact arm");
    M::decode(any.value.as_slice()).expect("the fact's type")
}

// --- a saga translates an event into a command --------------------------------------

#[test]
fn a_saga_translates_an_event_into_a_command_for_its_destination() {
    let resp = buyin_wallet()
        .dispatch(&trigger_book(None))
        .expect("translated");
    assert_eq!(resp.commands.len(), 1);
    assert_eq!(
        crate::command_payload(resp.commands[0].page.as_ref().unwrap()),
        Some(&any_of(&HoldFunds { amount: 150 }))
    );
}

// A saga stays in the edition of the event it handles, and so do its
// commands (types.proto, Cover.edition, What-if execution).
#[test]
fn a_sagas_commands_stay_in_their_triggers_edition() {
    let resp = buyin_wallet()
        .dispatch(&trigger_book(Some("what-if")))
        .expect("translated");
    assert_eq!(
        resp.commands[0].cover.as_ref().unwrap().edition.as_deref(),
        Some("what-if")
    );
}

// --- pass-through ----------------------------------------------------------------------

#[test]
fn pass_through_answers_the_standard_command_refused() {
    let answer = buyin_wallet()
        .dispatch_refusal(&refusal(None, Some(insufficient())))
        .expect("answered");
    let fact: pb::CommandRefused = fact_of(&answer);
    assert_eq!(
        fact,
        pb::CommandRefused {
            command: Some(sent_command(None)),
            refusal: Some(insufficient()),
            trigger: Some(trigger_ref(None)),
        }
    );
    assert!(
        answer.fact.unwrap().cover.is_none(),
        "the saga leaves the cover to the runtime"
    );
}

#[test]
fn pass_through_without_a_typed_refusal_leaves_refusal_unset() {
    let answer = buyin_wallet()
        .dispatch_refusal(&refusal(None, None))
        .expect("answered");
    let fact: pb::CommandRefused = fact_of(&answer);
    assert_eq!(fact.refusal, None);
    assert_eq!(fact.trigger, Some(trigger_ref(None)));
}

// --- translate --------------------------------------------------------------------------

#[test]
fn translate_answers_the_callbacks_domestic_fact() {
    let saga = buyin_wallet().on_refused(&fq::<HoldFunds>(), |refusal| {
        let typed = refusal
            .response
            .as_ref()
            .and_then(|r| r.refusal.as_ref())
            .expect("the typed refusal");
        let insufficient =
            InsufficientFunds::decode(typed.value.as_slice()).expect("InsufficientFunds");
        Ok(any_of(&FundsUnavailable {
            available: insufficient.available,
        }))
    });
    let answer = saga
        .dispatch_refusal(&refusal(None, Some(insufficient())))
        .expect("answered");
    let fact: FundsUnavailable = fact_of(&answer);
    assert_eq!(fact, FundsUnavailable { available: 100 });
}

// The callback's presence is translate mode for THAT command type only.
#[test]
fn a_command_type_with_no_callback_is_passed_through() {
    let saga = buyin_wallet().on_refused(&fq::<ReleaseFunds>(), |_| {
        panic!("not the refused command's type")
    });
    let answer = saga
        .dispatch_refusal(&refusal(None, Some(insufficient())))
        .expect("answered");
    let fact: pb::CommandRefused = fact_of(&answer);
    assert_eq!(fact.refusal, Some(insufficient()));
}

// A failing callback is a saga bug: dead-lettered and alerted by the runtime.
#[test]
fn a_failing_callback_is_a_saga_bug() {
    let saga = buyin_wallet().on_refused(&fq::<HoldFunds>(), |_| {
        Err(HandlerError::Other("cannot translate".to_string()))
    });
    let err = saga
        .dispatch_refusal(&refusal(None, Some(insufficient())))
        .expect_err("a bug");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(err.grpc, GrpcCode::Internal);
}

// --- addressing and identity ----------------------------------------------------------

#[test]
fn the_refusal_fact_goes_to_the_triggers_exact_stream() {
    let refusal = refusal(Some("what-if"), Some(insufficient()));
    let answer = buyin_wallet().dispatch_refusal(&refusal).expect("answered");
    let fact = address_refusal_fact(&refusal, &answer).expect("addressed");
    assert_eq!(fact.cover, Some(stream("buyin", Some("what-if"), INSTANCE)));
}

// One id in two streams by design: never applied at the target, recorded as
// the refusal at the source; both deduplicate on it.
#[test]
fn the_refusal_facts_id_is_the_refused_commands_id() {
    let refusal = refusal(None, Some(insufficient()));
    let answer = buyin_wallet().dispatch_refusal(&refusal).expect("answered");
    let fact = address_refusal_fact(&refusal, &answer).expect("addressed");
    let page = fact.page.as_ref().expect("its page");
    assert_eq!(page.command_id, Some(uuid_of(9)));
    assert_eq!(page.causation_id, Some(trigger_ref(None)));
    assert_eq!(page.correlation_id, Some(uuid_of(INSTANCE)));
    assert_eq!(
        page.fact_origin.as_ref().map(|o| o.external_id.as_str()),
        Some("09090909-0909-0909-0909-090909090909")
    );
    assert_eq!(crate::input_id(&fact), Some(uuid_of(9)));
}

// The saga builds Facts only through OnRefused: an answer that is not a Fact,
// or that sets its own cover, is a saga bug.
#[test]
fn an_answer_that_is_not_a_bare_fact_is_refused() {
    let refusal = refusal(None, Some(insufficient()));
    let not_a_fact = pb::OnRefusedResponse {
        fact: Some(pb::CommandBook {
            page: Some(command_page(any_of(&ReleaseFunds { amount: 1 }))),
            ..Default::default()
        }),
    };
    let addressed = pb::OnRefusedResponse {
        fact: Some(pb::CommandBook {
            cover: Some(stream("wallet", None, 1)),
            page: Some(fact_page(any_of(&FundsUnavailable { available: 1 }))),
            ..Default::default()
        }),
    };
    for answer in [not_a_fact, addressed] {
        let err: CodedError = address_refusal_fact(&refusal, &answer).expect_err("a saga bug");
        assert_eq!(err.grpc, GrpcCode::Internal);
    }
}
