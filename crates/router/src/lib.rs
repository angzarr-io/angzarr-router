//! angzarr-router — the shared client router core.
//!
//! The engine semantics table in client-go's docs/architecture.md is this
//! crate's contract: dispatch/rebuild mechanics implemented exactly once,
//! consumed natively by client-rust and through the C-ABI FFI crate by
//! every other language binding. Framework rules live here — do not
//! duplicate them into generated output, bindings, or component adapters.

pub mod aggregate;
pub mod destinations;
pub mod error;
pub mod projector;
pub mod proto;
pub mod rebuild;
pub mod saga;

pub use proto::io::angzarr::v1 as pb;

use prost_types::Any;

/// angzarr's canonical type-URL prefix: a bare `/` (the empty type-domain
/// the `Any` spec blesses, and prost's `Name::type_url()` default). The
/// segment after it is the fully-qualified proto name — no resolver host.
pub const TYPE_URL_PREFIX: &str = "/";

/// Fully-qualified proto name of the cross-domain rejection Notification.
pub const NOTIFICATION_FULL_NAME: &str = "io.angzarr.v1.Notification";

/// Canonical wire type_url angzarr PRODUCES for rejection notifications
/// (bare form). Other-language bindings stamp the same message with their
/// `Any.Pack()` default (`type.googleapis.com/...`); recognition therefore
/// matches the full FQN regardless of prefix — see [`is_notification_type_url`].
pub const NOTIFICATION_TYPE_URL: &str = "/io.angzarr.v1.Notification";

/// True iff `type_url` carries the Notification message, regardless of its
/// resolver prefix. Matches the FULL FQN (an absolute name), not a partial
/// suffix — so a user-defined `*.FooNotification` never misroutes.
pub fn is_notification_type_url(type_url: &str) -> bool {
    type_name_from_url(type_url) == NOTIFICATION_FULL_NAME
}

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
pub fn page_sequence(page: &pb::EventPage) -> u32 {
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

/// Extracts the source domain and FULLY-QUALIFIED command type name from
/// a RejectionNotification (FQ keys; short names never match).
pub fn extract_rejection_key(rejection: &pb::RejectionNotification) -> (String, String) {
    let Some(rejected) = rejection.rejected_command.as_ref() else {
        return (String::new(), String::new());
    };
    let domain = rejected
        .cover
        .as_ref()
        .map(|c| c.domain.clone())
        .unwrap_or_default();
    let cmd_type = rejected
        .pages
        .first()
        .and_then(command_payload)
        .map(|cmd| type_name_from_url(&cmd.type_url).to_string())
        .unwrap_or_default();
    (domain, cmd_type)
}

#[cfg(test)]
#[path = "test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "lib.test.rs"]
mod lib_tests;
