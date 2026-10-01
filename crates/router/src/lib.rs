//! angzarr-router — the shared client router core.
//!
//! Dispatch/rebuild mechanics implemented once, exposed through the C-ABI
//! FFI crate (crates/router-ffi) to the language bindings that
//! angzarr-cli's generated code targets. The engine semantics table in
//! docs/architecture.md is this crate's contract. Framework rules live
//! here — do not duplicate them into generated output, bindings, or
//! component adapters.

pub mod aggregate;
pub mod destinations;
pub mod error;
pub mod process_manager;
pub mod projector;
pub mod proto;
pub mod rebuild;
pub mod router;
pub mod saga;
pub mod upcaster;

pub use proto::io::angzarr::v1 as pb;

use std::collections::HashMap;

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

/// Stamps every page of saga/PM-emitted `commands` as deferred: the page
/// header becomes `angzarr_deferred` recording the triggering event (its
/// book's `source_cover` and the event's `source_seq`) and the command's
/// position in this invocation's output (`command_index`). Any explicit
/// sequence a handler set is replaced — a deferred command never carries an
/// expected version; a per-command `sync_mode` is kept. `source_component`
/// is left for the coordinator, which stamps the registered component name.
pub fn stamp_deferred(
    commands: &mut [pb::CommandBook],
    source_cover: Option<&pb::Cover>,
    source_seq: u32,
) {
    for (index, cmd) in commands.iter_mut().enumerate() {
        for page in &mut cmd.pages {
            let header = page.header.get_or_insert_with(pb::PageHeader::default);
            header.sequence_type = Some(pb::page_header::SequenceType::AngzarrDeferred(
                pb::AngzarrDeferredSequence {
                    source: source_cover.cloned(),
                    source_seq,
                    source_component: String::new(),
                    command_index: index as u32,
                },
            ));
        }
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

/// Fully-qualified names of the two Notification payloads.
pub const REJECTION_NOTIFICATION_FULL_NAME: &str = "io.angzarr.v1.RejectionNotification";
pub const COMPENSATE_FULL_NAME: &str = "io.angzarr.v1.Compensate";

/// A decoded Notification payload: a rejected command (routed to
/// compensation handlers) or a Compensate (routed to the undo handler for its
/// command type).
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationPayload {
    Rejection(pb::RejectionNotification),
    Compensate(pb::Compensate),
}

/// Decodes a Notification page payload and discriminates its payload by the
/// message name after the last `/` of `payload.type_url`. A payload with no
/// type name (or no payload at all) reads as a RejectionNotification; any
/// other name is UNKNOWN_NOTIFICATION_PAYLOAD.
pub fn decode_notification(
    notification_any: &Any,
) -> Result<(pb::Notification, NotificationPayload), error::CodedError> {
    use error::{codes, extras, messages, CodedError};
    use prost::Message;

    let notification =
        pb::Notification::decode(notification_any.value.as_slice()).map_err(|_| {
            CodedError::invalid_argument(
                codes::NOTIFICATION_DECODE_FAILED,
                messages::NOTIFICATION_DECODE_FAILED,
                [(
                    extras::TYPE_URL.to_string(),
                    notification_any.type_url.clone(),
                )],
            )
        })?;
    let Some(payload) = notification.payload.as_ref() else {
        return Ok((
            notification,
            NotificationPayload::Rejection(Default::default()),
        ));
    };
    let payload = match type_name_from_url(&payload.type_url) {
        COMPENSATE_FULL_NAME => {
            let compensate = pb::Compensate::decode(payload.value.as_slice()).map_err(|_| {
                CodedError::invalid_argument(
                    codes::COMPENSATE_DECODE_FAILED,
                    messages::COMPENSATE_DECODE_FAILED,
                    [],
                )
            })?;
            NotificationPayload::Compensate(compensate)
        }
        "" | REJECTION_NOTIFICATION_FULL_NAME => {
            let rejection =
                pb::RejectionNotification::decode(payload.value.as_slice()).map_err(|_| {
                    CodedError::invalid_argument(
                        codes::REJECTION_NOTIFICATION_DECODE_FAILED,
                        messages::REJECTION_NOTIFICATION_DECODE_FAILED,
                        [],
                    )
                })?;
            NotificationPayload::Rejection(rejection)
        }
        _ => {
            return Err(CodedError::invalid_argument(
                codes::UNKNOWN_NOTIFICATION_PAYLOAD,
                messages::UNKNOWN_NOTIFICATION_PAYLOAD,
                [(extras::TYPE_URL.to_string(), payload.type_url.clone())],
            ));
        }
    };
    Ok((notification, payload))
}

/// Splits a `compensates` entry into its optional target-domain qualifier
/// and the fully-qualified command type: `"domain:fq.Type"` or `"fq.Type"`.
pub fn parse_compensation_key(key: &str) -> (Option<&str>, &str) {
    match key.split_once(':') {
        Some((domain, fq)) => (Some(domain), fq),
        None => (None, key),
    }
}

/// Looks up the compensation entry for a rejected command of type
/// `fq_command` that was sent to `target_domain`: a `"target_domain:fq"`
/// entry matches only that domain; an unqualified `"fq"` entry matches any.
pub fn compensation_lookup<'a, T>(
    entries: &'a HashMap<String, T>,
    target_domain: &str,
    fq_command: &str,
) -> Option<&'a T> {
    entries
        .get(&format!("{target_domain}:{fq_command}"))
        .or_else(|| entries.get(fq_command))
}

/// Refuses a set of `compensates` entries that lists one command type both
/// unqualified and domain-qualified (a type is listed once unqualified or
/// once per domain, never both).
pub fn validate_compensation_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), error::CodedError> {
    let mut unqualified = std::collections::HashSet::new();
    let mut qualified = std::collections::HashSet::new();
    for key in keys {
        match parse_compensation_key(key) {
            (None, fq) => unqualified.insert(fq),
            (Some(_), fq) => qualified.insert(fq),
        };
    }
    if let Some(fq) = unqualified.intersection(&qualified).next() {
        return Err(error::CodedError::invalid_argument(
            error::codes::AMBIGUOUS_COMPENSATION,
            error::messages::AMBIGUOUS_COMPENSATION,
            [(error::extras::COMMAND_TYPE.to_string(), fq.to_string())],
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "lib.test.rs"]
mod lib_tests;
