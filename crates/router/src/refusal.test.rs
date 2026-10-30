//! A policy on the current spec: its commands stay in its trigger's edition,
//! and a refusal of one of them comes back through OnRefused as the one
//! refusal fact for its own source domain, passed through or translated
//! (policy.proto, PolicyService, Refusals; types.proto, the refusal_facts and
//! command_identity regions; decision 14.31).

use prost::Message;
use prost_types::Any;

use crate::domain::CommandContext;
use crate::error::{codes, CodedError, GrpcCode, HandlerError};
use crate::pb;
use crate::policy::{address_refusal_fact, PolicyDispatch};
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

fn trigger_ref(edition: Option<&str>) -> pb::EventRef {
    pb::EventRef {
        stream: Some(stream("buyin", edition, INSTANCE)),
        sequence: 4,
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

// A policy stays in the edition of the event it handles, and so do its
// commands (types.proto, Cover.edition, What-if execution).
#[test]
fn a_policies_commands_stay_in_their_triggers_edition() {
    let resp = buyin_wallet()
        .dispatch(&trigger_book(Some("what-if")))
        .expect("translated");
    assert_eq!(
        commands_of(&resp)[0]
            .cover
            .as_ref()
            .unwrap()
            .edition
            .as_deref(),
        Some("what-if")
    );
}

// A command that names a different edition from its trigger's is a policy bug
// (owner ruling, 2026-10-28): the whole answer is refused INTERNAL, as a
// command outside the policy's destination is. An unset edition is stamped with
// the trigger's (above); only an explicit different one is refused, from main
// or from an edition alike.
#[test]
fn a_policy_command_naming_another_edition_is_refused() {
    let policy = PolicyDispatch::new("Stray", "buyin", "wallet").on_event(
        &fq::<BuyInStarted>(),
        |_event, _cover| {
            Ok(vec![pb::CommandBook {
                cover: Some(stream("wallet", Some("elsewhere"), 1)),
                page: Some(command_page(any_of(&HoldFunds { amount: 1 }))),
                ..Default::default()
            }])
        },
    );
    for trigger_edition in [None, Some("what-if")] {
        let err = policy
            .dispatch(&trigger_book(trigger_edition))
            .expect_err("another edition");
        assert_eq!(err.code, codes::POLICY_COMMAND_OUTSIDE_EDITION);
        assert_eq!(err.grpc, GrpcCode::Internal);
    }
    // Naming its trigger's own edition is not a different one.
    let own = PolicyDispatch::new("Own", "buyin", "wallet").on_event(
        &fq::<BuyInStarted>(),
        |_event, _cover| {
            Ok(vec![pb::CommandBook {
                cover: Some(stream("wallet", Some("what-if"), 1)),
                page: Some(command_page(any_of(&HoldFunds { amount: 1 }))),
                ..Default::default()
            }])
        },
    );
    assert!(own.dispatch(&trigger_book(Some("what-if"))).is_ok());
}

// --- pass-through ----------------------------------------------------------------------

#[test]
fn pass_through_answers_the_standard_command_refused() {
    let answer = buyin_wallet()
        .dispatch_refusal(
            &refusal(None, Some(insufficient())),
            &CommandContext::default(),
        )
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
        "the policy leaves the cover to the runtime"
    );
}

#[test]
fn pass_through_without_a_typed_refusal_leaves_refusal_unset() {
    let answer = buyin_wallet()
        .dispatch_refusal(&refusal(None, None), &CommandContext::default())
        .expect("answered");
    let fact: pb::CommandRefused = fact_of(&answer);
    assert_eq!(fact.refusal, None);
    assert_eq!(fact.trigger, Some(trigger_ref(None)));
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
fn a_command_type_with_no_callback_is_passed_through() {
    let policy = buyin_wallet().on_refused(&fq::<ReleaseFunds>(), |_, _| {
        panic!("not the refused command's type")
    });
    let answer = policy
        .dispatch_refusal(
            &refusal(None, Some(insufficient())),
            &CommandContext::default(),
        )
        .expect("answered");
    let fact: pb::CommandRefused = fact_of(&answer);
    assert_eq!(fact.refusal, Some(insufficient()));
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
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(err.grpc, GrpcCode::Internal);
}

// --- addressing and identity ----------------------------------------------------------

#[test]
fn the_refusal_fact_goes_to_the_triggers_exact_stream() {
    let refusal = refusal(Some("what-if"), Some(insufficient()));
    let answer = buyin_wallet()
        .dispatch_refusal(&refusal, &CommandContext::default())
        .expect("answered");
    let fact = address_refusal_fact(&refusal, &answer).expect("addressed");
    assert_eq!(fact.cover, Some(stream("buyin", Some("what-if"), INSTANCE)));
}

// One id in two streams by design: never applied at the target, recorded as
// the refusal at the source; both deduplicate on it.
#[test]
fn the_refusal_facts_id_is_the_refused_commands_id() {
    let refusal = refusal(None, Some(insufficient()));
    let answer = buyin_wallet()
        .dispatch_refusal(&refusal, &CommandContext::default())
        .expect("answered");
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

// The policy builds Facts only through OnRefused: an answer that is not a Fact,
// or that sets its own cover, is a policy bug.
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
        let err: CodedError = address_refusal_fact(&refusal, &answer).expect_err("a policy bug");
        assert_eq!(err.grpc, GrpcCode::Internal);
    }
}

// The fact's identity is the runtime's to set: a policy that sets any one
// piece of it is a policy bug.
#[test]
fn an_answer_that_sets_any_of_the_facts_identity_is_refused() {
    let refusal = refusal(None, Some(insufficient()));
    let answered = buyin_wallet()
        .dispatch_refusal(&refusal, &CommandContext::default())
        .expect("answered");
    let setters: [fn(&mut pb::CommandPage); 4] = [
        |p| p.command_id = Some(uuid_of(1)),
        |p| p.correlation_id = Some(uuid_of(1)),
        |p| p.causation_id = Some(trigger_ref(None)),
        |p| p.fact_origin = Some(pb::FactOrigin::default()),
    ];
    for set in setters {
        let mut answer = answered.clone();
        set(answer.fact.as_mut().unwrap().page.as_mut().unwrap());
        let err: CodedError = address_refusal_fact(&refusal, &answer).expect_err("a policy bug");
        // Decision 14.37: the condition is its own key; no text tells it apart.
        assert_eq!(err.code, "REFUSAL_FACT_IDENTIFIED");
        assert!(err.extras.is_empty(), "no other metadata: {:?}", err.extras);
    }
}

#[test]
fn a_policy_router_answers_a_refusal_and_no_other_router_does() {
    let refusal = refusal(None, Some(insufficient()));
    let router = crate::router::RouterBuilder::new()
        .policy(buyin_wallet())
        .build()
        .expect("build");
    let answer = router
        .dispatch_refusal(&refusal, &CommandContext::default())
        .expect("answered");
    let fact: pb::CommandRefused = fact_of(&answer);
    assert_eq!(fact.refusal, Some(insufficient()));

    let projector = crate::router::RouterBuilder::new()
        .projector(crate::projector::ProjectorDispatch::new("p", || ()))
        .build()
        .expect("build");
    let err = projector
        .dispatch_refusal(&refusal, &CommandContext::default())
        .expect_err("no policy");
    assert_eq!(err.code, "NO_COMPONENT_OF_KIND");
}
