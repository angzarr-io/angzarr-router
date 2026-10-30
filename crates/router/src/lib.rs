//! angzarr-router — the shared client router core.
//!
//! Dispatch/rebuild mechanics implemented once, exposed through the C-ABI
//! FFI crate (crates/router-ffi) to the language bindings that
//! angzarr-cli's generated code targets. The engine semantics table in
//! docs/architecture.md is this crate's contract. Framework rules live
//! here — do not duplicate them into generated output, bindings, or
//! component adapters.

#![forbid(unsafe_code)]

pub mod context;
pub mod domain;
pub mod error;
pub mod policy;
pub mod projector;
pub mod proto;
pub mod rebuild;
pub mod router;

pub use proto::io::angzarr::v1 as pb;

/// angzarr-project's wire-type crate, re-exported so a dependent names the
/// very `angzarr_proto` this router was built with (its types are `pb`'s).
pub use angzarr_proto;

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
pub fn next_sequence(book: Option<&pb::EventBook>) -> u64 {
    book.map_or(0, |b| b.next_sequence)
}

/// The event payload of a page, when the page carries one.
pub fn page_event(page: &pb::EventPage) -> Option<&Any> {
    page.payload.as_ref()
}

/// The payload of a command page's Command arm, when the page carries one.
pub fn command_payload(page: &pb::CommandPage) -> Option<&Any> {
    match &page.body {
        Some(pb::command_page::Body::Command(command)) => command.payload.as_ref(),
        _ => None,
    }
}

/// The payload of a command page's Fact arm, when the page carries one.
pub fn fact_payload(page: &pb::CommandPage) -> Option<&Any> {
    match &page.body {
        Some(pb::command_page::Body::Fact(fact)) => fact.payload.as_ref(),
        _ => None,
    }
}

/// Decodes bytes a host handed the router as the message `M`: bytes that do
/// not decode are ANY_DECODE_FAILED naming `M` as `message_type`
/// ([`error::CodedError::undecodable`]; decision 14.37), one condition
/// whatever the message.
pub fn decode_message<M: prost::Message + prost::Name + Default>(
    bytes: &[u8],
) -> Result<M, error::CodedError> {
    M::decode(bytes).map_err(|e| error::CodedError::undecodable(&M::full_name(), &e))
}

/// Where a folded event sits: its book's cover (domain, root) and the page's
/// sequence. Projector folds and
/// state appliers receive it, e.g. for idempotent per-page handling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageContext<'a> {
    pub cover: Option<&'a pb::Cover>,
    pub sequence: u64,
    /// The page's metadata (types.proto, EventMeta), as delivered: its
    /// correlation_id, recorded_at and causation; `None` when the page
    /// carries none (an event this command just emitted carries none yet).
    /// A policy reads its trigger's correlation here (decision 14.36: it
    /// may name that correlation as the root).
    pub meta: Option<&'a pb::EventMeta>,
}

#[cfg(test)]
#[path = "test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "lib.test.rs"]
mod lib_tests;

#[cfg(test)]
#[path = "handling.test.rs"]
mod handling_tests;

#[cfg(test)]
#[path = "snapshot.test.rs"]
mod snapshot_tests;

#[cfg(test)]
#[path = "chain.test.rs"]
mod chain_tests;
