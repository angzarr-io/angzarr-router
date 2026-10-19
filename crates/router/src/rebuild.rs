//! Rebuilder — state reconstruction from an EventBook: snapshot first
//! (when configured), then every event page in order.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::{messages, CodedError, FoldAt};
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
/// Error semantics: a page that fails to fold FAILS the rebuild —
/// commands must never validate against silently-truncated state. The
/// events were accepted by the same apply logic, so the failure is a handler
/// bug (UNHANDLED_HANDLER_ERROR, INTERNAL; [`fold_error`]). A snapshot the
/// loader cannot load is PERSISTED_EVENT_CORRUPT. Event types with no
/// registered applier are skipped: not every event folds into state.
pub struct Rebuilder<S> {
    factory: Box<dyn Fn() -> S + Send + Sync>,
    snapshot: Option<SnapshotFn<S>>,
    appliers: HashMap<String, ApplierFn<S>>,
    fold: Option<FoldFn<S>>,
    /// The state message's type URL ("" when undeclared).
    state_type_url: String,
}

/// How a batched fold failed: the index of the failing page within the
/// batch it was handed, when it knows, and its own text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoldFailure {
    pub page_index: Option<usize>,
    pub message: String,
}

/// Where the pages a fold failed on came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOrigin {
    /// Persisted events: a rebuild, or a conflict window.
    Persisted,
    /// The events the command being handled emitted, not yet persisted.
    Emitted,
}

impl PageOrigin {
    fn message(self) -> &'static str {
        match self {
            PageOrigin::Persisted => messages::FOLD_FAILED,
            PageOrigin::Emitted => messages::FOLD_FAILED_EMITTED,
        }
    }
}

/// The error for a fold of `pages` (from `origin`) into state of
/// `state_type_url` that failed with `failure`: names the page
/// `failure.page_index` indexes, or the batch's sequence range when the
/// index is missing or outside the batch.
pub fn fold_error(
    origin: PageOrigin,
    pages: &[pb::EventPage],
    failure: &FoldFailure,
    state_type_url: &str,
) -> CodedError {
    let at = match failure.page_index.and_then(|index| pages.get(index)) {
        Some(page) => return page_fold_error(origin, page, state_type_url, &failure.message),
        None => FoldAt::Range {
            first: pages.first().map_or(0, |p| p.sequence),
            last: pages.last().map_or(0, |p| p.sequence),
        },
    };
    CodedError::fold_failed(origin.message(), at, state_type_url, &failure.message)
}

/// The type URL a component declares for the message `full_name`: "" (none
/// declared) for an empty name.
pub fn declared_type_url(full_name: &str) -> String {
    if full_name.is_empty() {
        String::new()
    } else {
        crate::type_url(full_name)
    }
}

/// The error for a fold that failed on `page`.
pub fn page_fold_error(
    origin: PageOrigin,
    page: &pb::EventPage,
    state_type_url: &str,
    fold_message: &str,
) -> CodedError {
    let type_url = crate::page_event(page).map_or("", |e| e.type_url.as_str());
    CodedError::fold_failed(
        origin.message(),
        FoldAt::Page {
            sequence: page.sequence,
            type_url,
        },
        state_type_url,
        fold_message,
    )
}

/// Folds a batch of event pages (with their book's cover) into state in one
/// call: the batched alternative to per-type appliers, for state the router
/// holds on a host's behalf (router-owned state across the C ABI).
pub type FoldFn<S> = Box<
    dyn Fn(&mut S, &[pb::EventPage], Option<&pb::Cover>) -> Result<(), FoldFailure> + Send + Sync,
>;

impl<S> Rebuilder<S> {
    /// A Rebuilder whose fresh state comes from `factory`.
    pub fn new(factory: impl Fn() -> S + Send + Sync + 'static) -> Self {
        Rebuilder {
            factory: Box::new(factory),
            snapshot: None,
            appliers: HashMap::new(),
            fold: None,
            state_type_url: String::new(),
        }
    }

    /// A Rebuilder starting from `S::default()` whose state type comes from
    /// `S`'s prost name: nobody declares the string by hand.
    pub fn of_message() -> Self
    where
        S: prost::Name + Default + 'static,
    {
        Rebuilder::new(S::default).with_state_type(&S::full_name())
    }

    /// Declares the state message's fully-qualified name, for state held as
    /// something other than its own message (an `Any` across the C ABI).
    /// An empty name declares none.
    pub fn with_state_type(mut self, full_name: &str) -> Self {
        self.state_type_url = declared_type_url(full_name);
        self
    }

    /// The declared state type URL ("" when undeclared).
    pub fn state_type_url(&self) -> &str {
        &self.state_type_url
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
    /// cover) in a single call, and none when there are none. Its failure
    /// fails the rebuild as a handler bug ([`fold_error`]); it is never
    /// called again for the same pages.
    pub fn with_fold(
        mut self,
        fold: impl Fn(&mut S, &[pb::EventPage], Option<&pb::Cover>) -> Result<(), FoldFailure>
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
                fold(&mut state, &uncovered, book.cover.as_ref()).map_err(|failure| {
                    fold_error(
                        PageOrigin::Persisted,
                        &uncovered,
                        &failure,
                        &self.state_type_url,
                    )
                })?;
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
            if let Err(err) = thunk(&mut state, event, &ctx) {
                return Err(page_fold_error(
                    PageOrigin::Persisted,
                    page,
                    &self.state_type_url,
                    &err.to_string(),
                ));
            }
            info.applied_count += 1;
        }
        Ok((state, info))
    }
}

impl<S> Rebuilder<S> {
    /// Folds `pages` (from `origin`) of a book with `cover` into `state`:
    /// through the batched fold in one call when one is registered (none for
    /// no pages), otherwise page by page through the appliers. A failure is
    /// a handler bug ([`fold_error`]) whose message says where the pages
    /// came from.
    pub fn apply_pages(
        &self,
        state: &mut S,
        pages: &[pb::EventPage],
        cover: Option<&pb::Cover>,
        origin: PageOrigin,
    ) -> Result<(), CodedError> {
        if let Some(fold) = self.fold.as_ref() {
            if pages.is_empty() {
                return Ok(());
            }
            return fold(state, pages, cover)
                .map_err(|failure| fold_error(origin, pages, &failure, &self.state_type_url));
        }
        for page in pages {
            self.fold_page(state, page, cover, origin)?;
        }
        Ok(())
    }

    /// Folds one persisted page of a book with `cover` into `state` through
    /// the batched fold when one is registered, else its applier, returning
    /// whether anything ran. A page with no event or an unapplied type is
    /// skipped; a failing fold is a handler bug ([`page_fold_error`]).
    pub fn apply_page(
        &self,
        state: &mut S,
        page: &pb::EventPage,
        cover: Option<&pb::Cover>,
    ) -> Result<bool, CodedError> {
        self.fold_page(state, page, cover, PageOrigin::Persisted)
    }

    fn fold_page(
        &self,
        state: &mut S,
        page: &pb::EventPage,
        cover: Option<&pb::Cover>,
        origin: PageOrigin,
    ) -> Result<bool, CodedError> {
        if let Some(fold) = self.fold.as_ref() {
            let pages = std::slice::from_ref(page);
            fold(state, pages, cover)
                .map_err(|failure| fold_error(origin, pages, &failure, &self.state_type_url))?;
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
        thunk(state, event, &ctx)
            .map_err(|err| page_fold_error(origin, page, &self.state_type_url, &err.to_string()))?;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "rebuild.test.rs"]
mod rebuild_tests;
