//! angzarr-router — the shared client router core.
//!
//! Dispatch/rebuild mechanics implemented once, exposed through the C-ABI
//! FFI crate (crates/router-ffi) to the language bindings that
//! angzarr-cli's generated code targets. The engine semantics table in
//! docs/architecture.md is this crate's contract. Framework rules live
//! here — do not duplicate them into generated output, bindings, or
//! component adapters.

#![forbid(unsafe_code)]

pub mod conflict;
pub mod descriptor;
pub mod domain;
pub mod error;
pub mod projector;
pub mod proto;
pub mod rebuild;
pub mod router;
pub mod saga;
pub mod upcaster;

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

/// An input's deduplication key on its addressed stream (types.proto, the
/// command_identity region): a command's `command_id`; a fact's
/// `command_id` when set, else UUIDv5 of its `fact_origin.external_id`
/// (`angzarr_proto::ids::fact_command_id`); a refusal fact's is the refused
/// command's id, which the saga's runtime stamped as its `command_id`.
/// `None` when the input carries no identity to key on.
///
/// There is no trigger-fed process manager to key by trigger (decision
/// 14.31: a process manager is a domain, deduplicated like any other):
///
/// ```compile_fail
/// use angzarr_router::process_manager::ProcessManagerDispatch;
/// ```
pub fn input_id(book: &pb::CommandBook) -> Option<pb::Uuid> {
    let page = book.page.as_ref()?;
    if let Some(id) = page.command_id.as_ref() {
        return Some(id.clone());
    }
    match &page.body {
        Some(pb::command_page::Body::Fact(_)) => page
            .fact_origin
            .as_ref()
            .filter(|origin| !origin.external_id.is_empty())
            .map(|origin| angzarr_proto::ids::fact_command_id(&origin.external_id)),
        _ => None,
    }
}

/// An upcaster chain a command handler runs on incoming command and fact
/// payloads before dispatch (upcaster.proto; decision 12.11):
/// [`upcaster::UpcasterDispatch`] is one.
pub trait PayloadUpcaster: Send + Sync {
    /// The payload brought up to its current type (unchanged when no rule
    /// matches).
    fn upcast_payload(&self, payload: &Any) -> Result<Any, error::CodedError>;
    /// The fully-qualified source types the chain accepts: claimed for its
    /// aggregate at registration, like command types.
    fn source_types(&self) -> Vec<String>;
}

/// Where a folded event sits: its book's cover (domain, root) and the page's
/// sequence. Projector folds and
/// state appliers receive it, e.g. for idempotent per-page handling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageContext<'a> {
    pub cover: Option<&'a pb::Cover>,
    pub sequence: u64,
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
