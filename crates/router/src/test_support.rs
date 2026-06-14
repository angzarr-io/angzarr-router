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

pub fn event_page(event: Any) -> pb::EventPage {
    pb::EventPage {
        payload: Some(pb::event_page::Payload::Event(event)),
        ..Default::default()
    }
}

pub fn sequenced_event_page(seq: u32, event: Any) -> pb::EventPage {
    pb::EventPage {
        header: Some(pb::PageHeader {
            sequence_type: Some(pb::page_header::SequenceType::Sequence(seq)),
            ..Default::default()
        }),
        payload: Some(pb::event_page::Payload::Event(event)),
        ..Default::default()
    }
}

/// A book of Cover events, one per domain, in order.
pub fn book_of_covers(domains: &[&str]) -> pb::EventBook {
    pb::EventBook {
        pages: domains.iter().map(|d| event_page(cover_any(d))).collect(),
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

/// A Notification event page carrying a RejectionNotification for the
/// given fully-qualified command type (rejected by domain "inventory").
pub fn notification_page_for(fq_command: &str) -> pb::EventPage {
    let rejection = pb::RejectionNotification {
        rejected_command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: "inventory".to_string(),
                ..Default::default()
            }),
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(Any {
                    type_url: format!("{TYPE_URL_PREFIX}{fq_command}"),
                    value: Vec::new(),
                })),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    let notification = pb::Notification {
        payload: Some(any_of(&rejection)),
        ..Default::default()
    };
    event_page(any_of(&notification))
}

/// Wraps a command Any as the first page of a ContextualCommand.
pub fn command_for(command: Any) -> pb::ContextualCommand {
    pb::ContextualCommand {
        command: Some(pb::CommandBook {
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(command)),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
}
