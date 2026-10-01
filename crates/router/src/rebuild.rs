//! Rebuilder — state reconstruction from an EventBook: snapshot first
//! (when configured), then every event page in order.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::CodedError;
use crate::pb;

/// Folds one event payload into state. Generated/binding thunks unmarshal
/// to the typed event and call the pure typed applier; decode errors
/// surface here so the engine can classify them.
pub type ApplierFn<S> =
    Box<dyn Fn(&mut S, &Any) -> Result<(), Box<dyn std::error::Error + Send + Sync>> + Send + Sync>;

/// What the rebuild consumed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildInfo {
    /// True when the book carried any history (pages or a snapshot) — the
    /// "does this aggregate exist" signal. Reflects page presence, not
    /// applier hits.
    pub had_prior_events: bool,
    /// Number of pages whose type had a registered applier.
    pub applied_count: usize,
}

/// Reconstructs state of type S from an EventBook.
///
/// Error semantics: a persisted payload that fails to decode FAILS the
/// rebuild with PERSISTED_EVENT_CORRUPT — commands must never validate
/// against silently-truncated state. Event types with no registered
/// applier are skipped: not every event folds into state.
pub struct Rebuilder<S> {
    factory: Box<dyn Fn() -> S + Send + Sync>,
    snapshot: Option<ApplierFn<S>>,
    appliers: HashMap<String, ApplierFn<S>>,
}

impl<S> Rebuilder<S> {
    /// A Rebuilder whose fresh state comes from `factory`.
    pub fn new(factory: impl Fn() -> S + Send + Sync + 'static) -> Self {
        Rebuilder {
            factory: Box::new(factory),
            snapshot: None,
            appliers: HashMap::new(),
        }
    }

    /// Registers the applier for a fully-qualified event type name.
    pub fn apply(
        mut self,
        full_name: &str,
        thunk: impl Fn(&mut S, &Any) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.appliers.insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// Registers the snapshot loader, applied before any pages.
    pub fn with_snapshot(
        mut self,
        thunk: impl Fn(&mut S, &Any) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.snapshot = Some(Box::new(thunk));
        self
    }

    /// Folds the book into fresh state. An absent book normalizes to an
    /// empty one (fresh state, no prior events) — callers never need
    /// their own guard.
    pub fn rebuild(&self, book: Option<&pb::EventBook>) -> Result<(S, RebuildInfo), CodedError> {
        let mut state = (self.factory)();
        let mut info = RebuildInfo::default();
        let Some(book) = book else {
            return Ok((state, info));
        };
        info.had_prior_events = !book.pages.is_empty() || book.snapshot.is_some();

        // Pages a LOADED snapshot already folded must not re-apply — their
        // effects are in the snapshot state; double-folding corrupts it. A
        // snapshot that did not load (no loader, no state) covers nothing, and a
        // page without an explicit sequence is never provably covered.
        let mut covered_through: Option<u32> = None;
        if let Some(snapshot) = book.snapshot.as_ref() {
            if let (Some(snap_state), Some(loader)) =
                (snapshot.state.as_ref(), self.snapshot.as_ref())
            {
                if loader(&mut state, snap_state).is_err() {
                    return Err(CodedError::persisted_corrupt(&snap_state.type_url));
                }
                covered_through = Some(snapshot.sequence);
            }
        }

        for page in &book.pages {
            let Some(event) = crate::page_event(page) else {
                continue;
            };
            if let (Some(covered), Some(seq)) = (covered_through, explicit_sequence(page)) {
                if seq <= covered {
                    continue;
                }
            }
            let Some(thunk) = self
                .appliers
                .get(crate::type_name_from_url(&event.type_url))
            else {
                continue;
            };
            if thunk(&mut state, event).is_err() {
                return Err(CodedError::persisted_corrupt(&event.type_url));
            }
            info.applied_count += 1;
        }
        Ok((state, info))
    }
}

/// The page's explicit header sequence, when it carries one.
fn explicit_sequence(page: &pb::EventPage) -> Option<u32> {
    match page.header.as_ref()?.sequence_type.as_ref()? {
        pb::page_header::SequenceType::Sequence(seq) => Some(*seq),
        _ => None,
    }
}

#[cfg(test)]
#[path = "rebuild.test.rs"]
mod rebuild_tests;
