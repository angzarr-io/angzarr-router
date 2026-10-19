//! angzarr-router — the shared client router core.
//!
//! Dispatch/rebuild mechanics implemented once, exposed through the C-ABI
//! FFI crate (crates/router-ffi) to the language bindings that
//! angzarr-cli's generated code targets. The engine semantics table in
//! docs/architecture.md is this crate's contract. Framework rules live
//! here — do not duplicate them into generated output, bindings, or
//! component adapters.

#![forbid(unsafe_code)]

pub mod aggregate;
pub mod error;
pub mod process_manager;
pub mod projector;
pub mod proto;
pub mod rebuild;
pub mod router;
pub mod saga;
pub mod upcaster;

pub use proto::io::angzarr::v1 as pb;

use prost_types::Any;

/// angzarr's canonical type-URL prefix: a bare `/` (the empty type-domain
/// the `Any` spec blesses, and prost's `Name::type_url()` default). The
/// segment after it is the fully-qualified proto name — no resolver host.
pub const TYPE_URL_PREFIX: &str = "/";

/// The domain a projector declares to consume every domain.
pub const WILDCARD_DOMAIN: &str = "*";

/// Constructs a canonical (bare) type URL from a fully-qualified type name.
pub fn type_url(full_name: &str) -> String {
    format!("{TYPE_URL_PREFIX}{full_name}")
}

/// Extracts the fully qualified type name from a type URL.
/// For "type.googleapis.com/examples.CardsDealt", returns "examples.CardsDealt".
pub fn type_name_from_url(type_url: &str) -> &str {
    match type_url.rfind('/') {
        Some(idx) => &type_url[idx + 1..],
        None => type_url,
    }
}

/// Returns the next sequence number from an EventBook.
///
/// The framework precomputes next_sequence on load (snapshots mean
/// counting pages gives the wrong answer); handlers MUST use this value.
pub fn next_sequence(book: Option<&pb::EventBook>) -> u32 {
    book.map_or(0, |b| b.next_sequence)
}

/// The event payload of a page, when the page carries one.
pub fn page_event(page: &pb::EventPage) -> Option<&Any> {
    match &page.payload {
        Some(pb::event_page::Payload::Event(any)) => Some(any),
        _ => None,
    }
}

/// The explicit sequence of a page header, or 0 when absent.
pub(crate) fn page_sequence(page: &pb::EventPage) -> u32 {
    match page.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
        Some(pb::page_header::SequenceType::Sequence(seq)) => *seq,
        _ => 0,
    }
}

/// The command payload of a command page, when the page carries one.
pub fn command_payload(page: &pb::CommandPage) -> Option<&Any> {
    match &page.payload {
        Some(pb::command_page::Payload::Command(any)) => Some(any),
        _ => None,
    }
}

/// Where a folded event sits: its book's cover (domain, root) and the page's
/// explicit sequence (0 when the page carries none). Projector folds and
/// state appliers receive it, e.g. for idempotent per-page handling or to
/// record which sequence produced a fact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageContext<'a> {
    pub cover: Option<&'a pb::Cover>,
    pub sequence: u32,
}

#[cfg(test)]
#[path = "test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "lib.test.rs"]
mod lib_tests;
