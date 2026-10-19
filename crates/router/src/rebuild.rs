//! Rebuilder — state reconstruction from an EventBook: snapshot first
//! (when configured), then every event page in order.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::CodedError;
use crate::pb;
use crate::PageContext;

/// Folds one event payload into state, seeing where the event sits.
/// Generated/binding thunks unmarshal to the typed event and call the pure
/// typed applier; decode errors surface here so the engine can classify them.
pub type ApplierFn<S> = Box<
    dyn for<'a> Fn(
            &mut S,
            &Any,
            &PageContext<'a>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + Send
        + Sync,
>;

/// Loads snapshot state into fresh state.
pub type SnapshotFn<S> =
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
    snapshot: Option<SnapshotFn<S>>,
    appliers: HashMap<String, ApplierFn<S>>,
    fold: Option<FoldFn<S>>,
}

/// Folds a batch of event pages (with their book's cover) into state in one
/// call: the batched alternative to per-type appliers, for state the router
/// holds on a host's behalf (router-owned state across the C ABI).
pub type FoldFn<S> = Box<
    dyn Fn(&mut S, &[pb::EventPage], Option<&pb::Cover>) -> Result<(), CodedError> + Send + Sync,
>;

impl<S> Rebuilder<S> {
    /// A Rebuilder whose fresh state comes from `factory`.
    pub fn new(factory: impl Fn() -> S + Send + Sync + 'static) -> Self {
        Rebuilder {
            factory: Box::new(factory),
            snapshot: None,
            appliers: HashMap::new(),
            fold: None,
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
        self.appliers.insert(
            full_name.to_string(),
            Box::new(move |state, any, _ctx| thunk(state, any)),
        );
        self
    }

    /// Registers an applier that also sees the page's context (the book's
    /// cover and the page's sequence).
    pub fn apply_with_context(
        mut self,
        full_name: &str,
        thunk: impl for<'a> Fn(
                &mut S,
                &Any,
                &PageContext<'a>,
            ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.appliers.insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// Folds pages through one batched `fold` instead of the per-type
    /// appliers: a rebuild hands it every uncovered page (and the book's
    /// cover) in a single call, and none when there are none. Its error
    /// fails the rebuild as returned.
    pub fn with_fold(
        mut self,
        fold: impl Fn(&mut S, &[pb::EventPage], Option<&pb::Cover>) -> Result<(), CodedError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.fold = Some(Box::new(fold));
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
        // snapshot that did not load (no loader, no state) covers nothing.
        // Every persisted page carries its definite sequence.
        let mut covered_through: Option<u64> = None;
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

        if let Some(fold) = self.fold.as_ref() {
            let uncovered: Vec<pb::EventPage> = book
                .pages
                .iter()
                .filter(|p| covered_through.is_none_or(|covered| p.sequence > covered))
                .cloned()
                .collect();
            if !uncovered.is_empty() {
                fold(&mut state, &uncovered, book.cover.as_ref())?;
                info.applied_count = uncovered.len();
            }
            return Ok((state, info));
        }

        for page in &book.pages {
            let Some(event) = crate::page_event(page) else {
                continue;
            };
            if covered_through.is_some_and(|covered| page.sequence <= covered) {
                continue;
            }
            let Some(thunk) = self
                .appliers
                .get(crate::type_name_from_url(&event.type_url))
            else {
                continue;
            };
            let ctx = PageContext {
                cover: book.cover.as_ref(),
                sequence: page.sequence,
            };
            if thunk(&mut state, event, &ctx).is_err() {
                return Err(CodedError::persisted_corrupt(&event.type_url));
            }
            info.applied_count += 1;
        }
        Ok((state, info))
    }
}

impl<S> Rebuilder<S> {
    /// Folds `pages` of a book with `cover` into `state`: through the
    /// batched fold in one call when one is registered (none for no pages),
    /// otherwise page by page through the appliers ([`Self::apply_page`]).
    pub fn apply_pages(
        &self,
        state: &mut S,
        pages: &[pb::EventPage],
        cover: Option<&pb::Cover>,
    ) -> Result<(), CodedError> {
        if let Some(fold) = self.fold.as_ref() {
            if pages.is_empty() {
                return Ok(());
            }
            return fold(state, pages, cover);
        }
        for page in pages {
            self.apply_page(state, page, cover)?;
        }
        Ok(())
    }

    /// Folds one page of a book with `cover` into `state` through the batched
    /// fold when one is registered, else its applier, returning whether
    /// anything ran. A page with no event or an unapplied type is skipped; a
    /// corrupt payload is PERSISTED_EVENT_CORRUPT.
    pub fn apply_page(
        &self,
        state: &mut S,
        page: &pb::EventPage,
        cover: Option<&pb::Cover>,
    ) -> Result<bool, CodedError> {
        if let Some(fold) = self.fold.as_ref() {
            fold(state, std::slice::from_ref(page), cover)?;
            return Ok(true);
        }
        let Some(event) = crate::page_event(page) else {
            return Ok(false);
        };
        let Some(thunk) = self
            .appliers
            .get(crate::type_name_from_url(&event.type_url))
        else {
            return Ok(false);
        };
        let ctx = PageContext {
            cover,
            sequence: page.sequence,
        };
        thunk(state, event, &ctx).map_err(|_| CodedError::persisted_corrupt(&event.type_url))?;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "rebuild.test.rs"]
mod rebuild_tests;
