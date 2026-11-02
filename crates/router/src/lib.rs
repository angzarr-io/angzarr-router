//! angzarr-router — the Rust client library's router, and the reference
//! router (decision 14.42).
//!
//! It keeps dispatch only: it routes an input by its type to business code,
//! folds the starting state and the pages it is given, runs decide or
//! ingest, reports a stale command's conflicts, runs a policy's handlers
//! per page and its translating callback, runs a projector, and turns
//! whatever escapes business code into a key and its args. Everything
//! generic beside it (upcasting, snapshots, admission, descriptor
//! validation, the reason catalog and classification, policy output
//! checks, refusal building, envelope guards, conflict judgement) is the
//! coordinator's. angzarr-project's handler-contract tier is its complete
//! contract, and every router in every language is held to it.

#![forbid(unsafe_code)]

pub mod context;
pub mod domain;
pub mod error;
pub mod policy;
pub mod projector;
pub mod rebuild;
pub mod router;
pub mod send;

/// The io.angzarr.v1 wire types: angzarr-core's angzarr-proto, which
/// compiles every message of the spec (the service protos' too:
/// `CommandHandlerResponse`, `CommandResponse`, `PolicyResponse`, `Refusal`,
/// `OnRefusedResponse`). The router generates none; `pb::X` is
/// `angzarr_proto::v1::X`, the one type under two names.
pub use angzarr_proto::v1 as pb;

/// angzarr-core's wire-type crate, re-exported so a dependent names the
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
/// not decode are ROUTER_REASON_ANY_DECODE_FAILED naming `M` as `message_type`
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
    /// The page's metadata (types.proto, EventMeta): its correlation_id,
    /// recorded_at and causation. The router stamps nothing; this is the
    /// page's `meta`, passed through, and `None` exactly when the page
    /// carries none. Who fills it depends on where the page comes from:
    ///
    /// - A persisted page (a rebuild's history, a projector's or a policy's
    ///   delivered events, a conflict window) carries the meta its
    ///   coordinator stamped when it recorded the event: the coordinator
    ///   guarantees it.
    /// - A page folded at decide time ([`rebuild::Rebuilder::apply_emitted`],
    ///   [`rebuild::PageOrigin::Emitted`]: the decision's own events, a
    ///   recorded fact's) carries what its pages carry when folded. The
    ///   router's own [`domain`] handler folds them as the handler built
    ///   them, and a handler's pages carry no meta unless it set one, so its
    ///   appliers see `None` there and the stamped meta on replay. A host that stamps the decision's own
    ///   events before folding them (angzarr-embedded's `event_meta`) hands
    ///   its appliers, at decide time, the meta replay will: the host
    ///   guarantees that, not the router.
    ///
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
