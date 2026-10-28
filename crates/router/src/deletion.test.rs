//! An edition's deletion at dispatch (spec 581d23e; decision 14.32 as
//! amended: meta/events.proto, EditionDeleted). Per stream, in order: the
//! deletion is RECORDED in the stream's own domain, PROPAGATED onward through
//! that domain's policy flows like any event, and only then are the
//! stream's rows of the edition REMOVED. After deletion an arrival in the
//! edition is refused NOT_FOUND and re-creates nothing.
//!
//! What dispatch owns: a policy carries the deletion to the streams it
//! reached (the runtime names them), a domain admits the carried deletion as
//! its own EditionDeleted to record, and any other arrival in a deleted
//! edition is refused. Recording, the lookup of reached streams and the
//! removal (the store's `remove_deleted`) are the runtime's.

use prost::{Message, Name};
use prost_types::Any;

use crate::domain::{Admission, Arrival, DomainDispatch, StreamStatus};
use crate::error::{codes, GrpcCode};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::test_support::*;

const X: &str = "what-if";
const INSTANCE: u8 = 7;

fn wallet() -> DomainDispatch<TestState> {
    DomainDispatch::new("Wallet", "wallet", fresh_rebuilder())
        .on_command(&fq::<HoldFunds>(), |_c, _s, _ctx| Ok(None))
}

fn buyin_wallet() -> PolicyDispatch {
    PolicyDispatch::new("BuyInWallet", "buyin", "wallet").on_event(
        &fq::<BuyInStarted>(),
        |_event, _cover| {
            Ok(vec![pb::CommandBook {
                cover: Some(stream("wallet", None, 1)),
                page: Some(command_page(any_of(&HoldFunds { amount: 1 }))),
                ..Default::default()
            }])
        },
    )
}

fn edition_deleted(commit_position: u64) -> pb::meta::EditionDeleted {
    pb::meta::EditionDeleted {
        edition: X.to_string(),
        commit_position,
        propagated_from: None,
        deleted_by: None,
    }
}

fn meta_any<M: Message + Name>(event: &M) -> Any {
    Any {
        type_url: crate::type_url(&M::full_name()),
        value: event.encode_to_vec(),
    }
}

/// The buy-in instance in X, which has recorded its EditionDeleted at
/// sequence 5.
fn deleted_source() -> pb::EventBook {
    pb::EventBook {
        cover: Some(stream("buyin", Some(X), INSTANCE)),
        pages: vec![sequenced_event_page(5, meta_any(&edition_deleted(42)))],
        ..Default::default()
    }
}

fn source_ref() -> pb::EventRef {
    pb::EventRef {
        stream: Some(stream("buyin", Some(X), INSTANCE)),
        sequence: 5,
    }
}

/// The deletion the policy carries to the wallet, as the runtime delivers
/// it: identity stamped (command_id 9).
fn carried_to_wallet() -> pb::CommandBook {
    let mut book = buyin_wallet()
        .carry_deletion(&deleted_source(), &[stream("wallet", Some(X), 1)])
        .expect("carried")
        .remove(0);
    book.page.as_mut().unwrap().command_id = Some(uuid_of(9));
    book
}

fn status(forked: bool, edition_deleted: bool, deleted: bool) -> StreamStatus {
    StreamStatus {
        exists: true,
        forked,
        next_sequence: 3,
        edition_deleted,
        deleted,
    }
}

fn admit(
    domain: &DomainDispatch<TestState>,
    command: &pb::CommandBook,
    stream: StreamStatus,
) -> Result<Admission, crate::error::CodedError> {
    domain.admit(&Arrival {
        command,
        recorded: &[],
        stream,
    })
}

// --- the meta-event namespace ------------------------------------------------

#[test]
fn a_meta_event_is_one_in_the_reserved_namespace() {
    assert!(crate::is_meta_event("/io.angzarr.v1.meta.EditionDeleted"));
    assert!(crate::is_meta_event("/io.angzarr.v1.meta.EditionCreated"));
    assert!(!crate::is_meta_event("/io.angzarr.v1.CommandRefused"));
    assert!(!crate::is_meta_event("/test.buyin.BuyInStarted"));
    assert!(!crate::is_meta_event("/io.angzarr.v1.metadata.Thing"));
}

// Business handlers never see a meta-event, even one whose type a handler
// claims.
#[test]
fn a_policy_never_hands_a_meta_event_to_business_code() {
    let policy = buyin_wallet()
        .on_event(&pb::meta::EditionDeleted::full_name(), |_event, _cover| {
            panic!("business code saw a meta-event")
        });
    let resp = policy.dispatch(&deleted_source()).expect("dispatched");
    assert!(resp.commands.is_empty());
}

// --- PROPAGATED: a policy carries the deletion onward ---------------------------

#[test]
fn a_policy_carries_a_deletion_to_each_stream_of_the_edition_it_reached() {
    let targets = [stream("wallet", Some(X), 1), stream("wallet", Some(X), 2)];
    let carried = buyin_wallet()
        .carry_deletion(&deleted_source(), &targets)
        .expect("carried");
    assert_eq!(carried.len(), 2);
    for (book, target) in carried.iter().zip(targets) {
        assert_eq!(book.cover.as_ref(), Some(&target));
        let page = book.page.as_ref().expect("a page");
        assert_eq!(page.causation_id, Some(source_ref()));
        assert_eq!(page.command_id, None, "identity is the runtime's");
        assert_eq!(
            crate::command_payload(page),
            Some(&meta_any(&pb::meta::EditionDeleted {
                propagated_from: Some(source_ref()),
                ..edition_deleted(42)
            }))
        );
    }
}

// The runtime names reached streams without their edition; the policy puts
// them in the deleted edition, and refuses one in another.
#[test]
fn a_carried_deletion_is_addressed_in_the_deleted_edition() {
    let carried = buyin_wallet()
        .carry_deletion(&deleted_source(), &[stream("wallet", None, 1)])
        .expect("carried");
    assert_eq!(
        carried[0].cover.as_ref().unwrap().edition.as_deref(),
        Some(X)
    );
    let err = buyin_wallet()
        .carry_deletion(&deleted_source(), &[stream("wallet", Some("other"), 1)])
        .expect_err("another edition");
    assert_eq!(err.code, codes::POLICY_COMMAND_OUTSIDE_EDITION);
}

#[test]
fn a_deletion_is_carried_only_into_the_policys_destination() {
    let err = buyin_wallet()
        .carry_deletion(&deleted_source(), &[stream("table", Some(X), 1)])
        .expect_err("outside the destination");
    assert_eq!(err.code, codes::POLICY_COMMAND_OUTSIDE_DESTINATION);
    assert_eq!(err.grpc, GrpcCode::Internal);
}

#[test]
fn a_source_with_no_deletion_carries_nothing() {
    let source = pb::EventBook {
        cover: Some(stream("buyin", Some(X), INSTANCE)),
        pages: vec![sequenced_event_page(4, any_of(&BuyInStarted { amount: 1 }))],
        ..Default::default()
    };
    assert!(buyin_wallet()
        .carry_deletion(&source, &[stream("wallet", Some(X), 1)])
        .expect("nothing")
        .is_empty());
}

// --- RECORDED: the target admits the carried deletion as its own ----------------

// Admitted while the edition is being deleted, never refused NOT_FOUND.
#[test]
fn a_carried_deletion_is_recorded_by_a_stream_of_the_edition() {
    assert_eq!(
        admit(&wallet(), &carried_to_wallet(), status(true, true, false)).expect("admitted"),
        Admission::Delete(pb::meta::EditionDeleted {
            propagated_from: Some(source_ref()),
            deleted_by: Some(uuid_of(9)),
            ..edition_deleted(42)
        })
    );
}

// A process manager's stream records it the same way: nothing PM-specific,
// and no start rule applies to a meta-event.
#[test]
fn a_process_manager_records_a_carried_deletion_like_any_domain() {
    let pm = DomainDispatch::process_manager("BuyIn", "wallet", fresh_rebuilder())
        .on_command(&fq::<HoldFunds>(), |_c, _s, _ctx| Ok(None))
        .starts(&fq::<HoldFunds>());
    let stream = StreamStatus {
        exists: false,
        ..status(true, true, false)
    };
    assert!(matches!(
        admit(&pm, &carried_to_wallet(), stream),
        Ok(Admission::Delete(_))
    ));
}

// Nothing of the edition there (never reached, or already removed): nothing
// to record, and the deletion never forks a stream.
#[test]
fn a_carried_deletion_reaching_a_stream_with_nothing_of_the_edition_records_nothing() {
    assert_eq!(
        admit(&wallet(), &carried_to_wallet(), status(false, true, false)).expect("skipped"),
        Admission::Skip
    );
}

#[test]
fn a_carried_deletion_reaching_a_stream_already_deleted_records_nothing() {
    assert_eq!(
        admit(&wallet(), &carried_to_wallet(), status(true, true, true)).expect("skipped"),
        Admission::Skip
    );
}

// --- after deletion: arrivals are refused NOT_FOUND --------------------------------

#[test]
fn a_policy_command_in_a_deleted_edition_is_refused_not_found_and_forks_nothing() {
    let command = caused_by(
        command_book(
            stream("wallet", Some(X), 1),
            any_of(&HoldFunds { amount: 1 }),
            9,
        ),
        stream("buyin", Some(X), INSTANCE),
        4,
    );
    for stream in [status(false, true, false), status(true, true, false)] {
        let err = admit(&wallet(), &command, stream).expect_err("deleted");
        assert_eq!(err.code, codes::EDITION_NOT_FOUND);
        assert_eq!(err.grpc, GrpcCode::NotFound);
    }
}

#[test]
fn an_outside_input_in_a_deleted_edition_is_refused_not_found() {
    let command = command_book(
        stream("wallet", Some(X), 1),
        any_of(&HoldFunds { amount: 1 }),
        9,
    );
    let fact = fact_book(
        stream("wallet", Some(X), 1),
        any_of(&BuyInTimedOut {}),
        "timer-1",
    );
    for input in [&command, &fact] {
        let err = admit(&wallet(), input, status(true, true, false)).expect_err("deleted");
        assert_eq!(err.code, codes::EDITION_NOT_FOUND);
        assert_eq!(err.grpc, GrpcCode::NotFound);
    }
}

// --- the chain: recorded, carried, recorded, then refused -------------------------

#[test]
fn a_deletion_travels_from_its_origin_through_a_policy_and_then_shuts_the_edition() {
    // The origin (the buy-in instance) has recorded its EditionDeleted; the
    // runtime looks up the wallet streams the policy reached in X.
    let carried = buyin_wallet()
        .carry_deletion(&deleted_source(), &[stream("wallet", None, 1)])
        .expect("carried");
    let mut to_wallet = carried[0].clone();
    to_wallet.page.as_mut().unwrap().command_id = Some(uuid_of(9));
    let Admission::Delete(recorded) =
        admit(&wallet(), &to_wallet, status(true, true, false)).expect("admitted")
    else {
        panic!("the wallet records its own EditionDeleted");
    };
    assert_eq!(recorded.edition, X);

    // The runtime recorded it at the wallet's head and, once propagated,
    // removed the wallet's rows of X. A later policy command there is refused.
    let later = caused_by(
        command_book(
            stream("wallet", Some(X), 1),
            any_of(&HoldFunds { amount: 1 }),
            10,
        ),
        stream("buyin", Some(X), INSTANCE),
        6,
    );
    let removed = StreamStatus {
        exists: false,
        forked: false,
        next_sequence: 0,
        edition_deleted: true,
        deleted: true,
    };
    assert_eq!(
        admit(&wallet(), &later, removed)
            .expect_err("the edition is gone")
            .code,
        codes::EDITION_NOT_FOUND
    );
}
