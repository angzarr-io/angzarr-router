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
