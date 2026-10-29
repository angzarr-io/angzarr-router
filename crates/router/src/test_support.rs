//! Shared fixtures for the engine test bank — transliterated from
//! client-go's engine_test.go helpers so the two suites pin identical
//! shapes.

use prost::{Message, Name};
use prost_types::Any;

use crate::pb;
use crate::rebuild::Rebuilder;
use crate::{type_url, TYPE_URL_PREFIX};

/// The fold-observing state the Go bank calls counterState: records which
/// payload domains applied, in order.
#[derive(Debug, Default)]
pub struct TestState {
    pub applied: Vec<String>,
}

/// Packs a message into an Any under the canonical googleapis prefix.
pub fn any_of<M: Message + prost::Name>(msg: &M) -> Any {
    Any {
        type_url: type_url(&M::full_name()),
        value: msg.encode_to_vec(),
    }
}

pub fn cover_full_name() -> String {
    pb::Cover::full_name()
}

pub fn cover_any(domain: &str) -> Any {
    any_of(&pb::Cover {
        domain: domain.to_string(),
        ..Default::default()
    })
}

/// A 16-byte UUID of `byte` repeated.
pub fn uuid_of(byte: u8) -> pb::Uuid {
    pb::Uuid {
        value: vec![byte; 16].into(),
    }
}

/// An event page at sequence 0.
pub fn event_page(event: Any) -> pb::EventPage {
    sequenced_event_page(0, event)
}

pub fn sequenced_event_page(seq: u64, event: Any) -> pb::EventPage {
    pb::EventPage {
        payload: Some(event),
        sequence: seq,
        ..Default::default()
    }
}

/// A book of Cover events, one per domain, in order, at sequences 0...
pub fn book_of_covers(domains: &[&str]) -> pb::EventBook {
    pb::EventBook {
        pages: domains
            .iter()
            .zip(0..)
            .map(|(d, seq)| sequenced_event_page(seq, cover_any(d)))
            .collect(),
        ..Default::default()
    }
}

/// An Any claiming the Cover type but carrying undecodable bytes.
pub fn corrupt_cover_any() -> Any {
    Any {
        type_url: format!("{TYPE_URL_PREFIX}{}", cover_full_name()),
        value: vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    }
}

/// The applier the Go bank calls coverApplier: decodes a Cover and records
/// its domain.
pub fn cover_applier(r: Rebuilder<TestState>) -> Rebuilder<TestState> {
    r.apply(&cover_full_name(), |s, payload| {
        let c = pb::Cover::decode(payload.value.as_slice())?;
        s.applied.push(c.domain);
        Ok(())
    })
}

pub fn fresh_rebuilder() -> Rebuilder<TestState> {
    Rebuilder::new(TestState::default)
}

/// A command page whose Command arm carries `command`.
pub fn command_page(command: Any) -> pb::CommandPage {
    pb::CommandPage {
        body: Some(pb::command_page::Body::Command(pb::Command {
            payload: Some(command),
        })),
        ..Default::default()
    }
}

/// A command page whose Fact arm carries `fact`.
pub fn fact_page(fact: Any) -> pb::CommandPage {
    pb::CommandPage {
        body: Some(pb::command_page::Body::Fact(pb::Fact {
            payload: Some(fact),
        })),
        ..Default::default()
    }
}

/// Wraps a command Any as the page of a ContextualCommand.
pub fn command_for(command: Any) -> pb::ContextualCommand {
    contextual(command_page(command))
}

/// Wraps a command page as the page of a ContextualCommand.
pub fn contextual(page: pb::CommandPage) -> pb::ContextualCommand {
    pb::ContextualCommand {
        command: Some(pb::CommandBook {
            page: Some(page),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A native state message: its type URL comes from its prost name.
#[derive(Clone, PartialEq, prost::Message)]
pub struct LedgerState {
    #[prost(uint32, tag = "1")]
    pub balance: u32,
}

impl prost::Name for LedgerState {
    const NAME: &'static str = "LedgerState";
    const PACKAGE: &'static str = "test.ledger";
}

/// Asserts `err` is a bug (owner ruling on decision 14.37, 2026-10-29):
/// the router's own error on malformed input is INTERNAL, alerted and
/// dead-lettered, and never reaches a policy's OnRefused.
pub fn assert_bug(err: &crate::error::CodedError) {
    use crate::error::{ErrorClass, GrpcCode};
    assert_eq!(
        err.grpc,
        GrpcCode::Internal,
        "{}: the router's own malformed-input error is a bug",
        err.code
    );
    assert_eq!(err.grpc.class(), ErrorClass::BugFatal, "{}", err.code);
    assert_eq!(
        err.refusal_response(),
        None,
        "{}: never a policy's OnRefused",
        err.code
    );
}

/// Asserts `err` is a failing fold answered INTERNAL as a handler bug, under
/// `code` (FOLD_FAILED or FOLD_FAILED_EMITTED; decision 14.37), the `located` metadata (sequence and type_url, or the batch's
/// first_sequence and last_sequence) and `state_type_url`.
pub fn assert_fold_failed(
    err: &crate::error::CodedError,
    code: &str,
    located: &[(&str, &str)],
    state_type_url: &str,
) {
    use crate::error::{extras, GrpcCode};
    assert_eq!(
        err.grpc,
        GrpcCode::Internal,
        "a failing fold is a handler bug"
    );
    assert_eq!(err.code, code);
    for (key, value) in located {
        assert_eq!(
            err.extras.get(*key).map(String::as_str),
            Some(*value),
            "{key}"
        );
    }
    assert_eq!(
        err.extras.get(extras::STATE_TYPE_URL).map(String::as_str),
        Some(state_type_url)
    );
}

// --- the buy-in workflow (decision 14.31's example, cut down) ---------------
//
// A "buyin" process manager, a "wallet" aggregate and the policies between
// them: the PM records BuyInStarted; the policy (buyin -> wallet) turns it
// into HoldFunds; the wallet may refuse HoldFunds with the typed refusal
// InsufficientFunds; the refusal comes back to the buy-in as a fact.

macro_rules! test_message {
    ($name:ident, $package:literal { $($field:ident : $ty:ty = $tag:literal),* }) => {
        #[derive(Clone, PartialEq, prost::Message)]
        pub struct $name {
            $(#[prost(uint32, tag = $tag)] pub $field: $ty,)*
        }
        impl prost::Name for $name {
            const NAME: &'static str = stringify!($name);
            const PACKAGE: &'static str = $package;
        }
    };
}

test_message!(StartBuyIn, "test.buyin" { amount: u32 = "1" });
test_message!(RecordFundsHeld, "test.buyin" { amount: u32 = "1" });
test_message!(BuyInStarted, "test.buyin" { amount: u32 = "1" });
test_message!(BuyInFailed, "test.buyin" { requested: u32 = "1" });
test_message!(BuyInTimedOut, "test.buyin" {});
test_message!(FundsUnavailable, "test.buyin" { available: u32 = "1" });
test_message!(HoldFunds, "test.wallet" { amount: u32 = "1" });
test_message!(ReleaseFunds, "test.wallet" { amount: u32 = "1" });
test_message!(InsufficientFunds, "test.wallet" { available: u32 = "1", requested: u32 = "2" });

// --- policy addressing (decision 14.36; policy_addressing.feature) ----------
//
// An "order" aggregate whose OrderPlaced feeds two policies: one to the
// "inventory" aggregate (ReserveStock), one to the "fulfillment" process
// manager (StartFulfillment).

test_message!(OrderPlaced, "test.order" { quantity: u32 = "1" });
test_message!(ReserveStock, "test.inventory" { quantity: u32 = "1" });
test_message!(StartFulfillment, "test.fulfillment" { quantity: u32 = "1" });

/// The fully-qualified name of a test message.
pub fn fq<M: prost::Name>() -> String {
    M::full_name()
}

/// A stream's cover: `domain`, `edition` (None for main), root `root`.
pub fn stream(domain: &str, edition: Option<&str>, root: u8) -> pb::Cover {
    pb::Cover {
        domain: domain.to_string(),
        root: Some(uuid_of(root)),
        edition: edition.map(str::to_string),
    }
}

/// A command book addressed to `cover`, its Command arm carrying `command`,
/// with command_id `id`.
pub fn command_book(cover: pb::Cover, command: Any, id: u8) -> pb::CommandBook {
    let mut page = command_page(command);
    page.command_id = Some(uuid_of(id));
    pb::CommandBook {
        cover: Some(cover),
        page: Some(page),
        ..Default::default()
    }
}

/// A fact book addressed to `cover`, its Fact arm carrying `fact`, from the
/// external id `external_id` (command_id left for the runtime to derive).
pub fn fact_book(cover: pb::Cover, fact: Any, external_id: &str) -> pb::CommandBook {
    let mut page = fact_page(fact);
    page.fact_origin = Some(pb::FactOrigin {
        external_id: external_id.to_string(),
        ..Default::default()
    });
    pb::CommandBook {
        cover: Some(cover),
        page: Some(page),
        ..Default::default()
    }
}

/// `book` as a policy's command: caused by the event at `sequence` of
/// `trigger`.
pub fn caused_by(mut book: pb::CommandBook, trigger: pb::Cover, sequence: u64) -> pb::CommandBook {
    book.page.as_mut().unwrap().causation_id = Some(pb::EventRef {
        stream: Some(trigger),
        sequence,
    });
    book
}

/// An event page at `sequence` recorded by the command `command_id`.
pub fn recorded_page(sequence: u64, event: Any, command_id: u8) -> pb::EventPage {
    pb::EventPage {
        payload: Some(event),
        sequence,
        meta: Some(pb::EventMeta {
            causation: Some(pb::event_meta::Causation::CommandId(uuid_of(command_id))),
            ..Default::default()
        }),
    }
}
