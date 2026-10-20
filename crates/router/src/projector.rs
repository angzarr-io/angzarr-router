//! ProjectorDispatch — the events → projection table.
//!
//! One projection instance per delivery folds every declared event page in
//! order (the instance is reused across pages — spec C-0086), then an
//! optional Finish packs it into the wire Projection (without one, the
//! Projection carries the cover, the projector's name and its next
//! sequence). Unlike the aggregate
//! there is no prior-state rebuild, no snapshot, and no rejection path: an
//! event with no registered handler is skipped with a warn; events whose book cover carries an undeclared domain
//! never fold, though Finish still runs.

use std::collections::{HashMap, HashSet};

use prost_types::Any;

use crate::error::{codes, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::{fold_error, page_fold_error, FoldFailure, PageOrigin};

pub use crate::PageContext;

/// Folds one delivered event page into the rebuilding projection. Generated
/// thunks unmarshal to the typed event and call the typed business method.
pub type EventFn<P> =
    Box<dyn for<'a> Fn(&mut P, &Any, &PageContext<'a>) -> Result<(), HandlerError> + Send + Sync>;

/// Packs the folded projection instance into the wire Projection. When
/// absent, dispatch returns a default Projection (cover + projector name).
pub type FinishFn<P> =
    Box<dyn Fn(&mut P, &pb::EventBook) -> Result<pb::Projection, HandlerError> + Send + Sync>;

/// Folds every page of a consumed book into the projection in one call: the
/// batched alternative to per-type folds, for a projection the router holds
/// on a host's behalf (router-owned projections across the C ABI).
pub type BatchFoldFn<P> =
    Box<dyn Fn(&mut P, &[pb::EventPage], &pb::Cover) -> Result<(), FoldFailure> + Send + Sync>;

/// The dispatch table for one projector component.
pub struct ProjectorDispatch<P> {
    name: String,
    factory: Box<dyn Fn() -> P + Send + Sync>,
    domains: Option<HashSet<String>>,
    handlers: HashMap<String, EventFn<P>>,
    fold: Option<BatchFoldFn<P>>,
    finish: Option<FinishFn<P>>,
    rules_version: String,
    /// The projection message's type URL ("" when undeclared).
    projection_type_url: String,
}

impl<P> ProjectorDispatch<P> {
    /// An empty projector table over a fresh-projection factory.
    pub fn new(name: impl Into<String>, factory: impl Fn() -> P + Send + Sync + 'static) -> Self {
        ProjectorDispatch {
            name: name.into(),
            factory: Box::new(factory),
            domains: None,
            handlers: HashMap::new(),
            fold: None,
            finish: None,
            rules_version: String::new(),
            projection_type_url: String::new(),
        }
    }

    /// Folds through one batched `fold` instead of the per-type folds: a
    /// delivery whose domain the projector consumes hands it every page (and
    /// the book's cover) in a single call, and none when there are none.
    pub fn with_fold(
        mut self,
        fold: impl Fn(&mut P, &[pb::EventPage], &pb::Cover) -> Result<(), FoldFailure>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.fold = Some(Box::new(fold));
        self
    }

    /// Declares the projection message's fully-qualified name, named by a
    /// failing fold's error (`state_type_url`). An empty name declares none.
    pub fn with_projection_type(mut self, full_name: &str) -> Self {
        self.projection_type_url = crate::rebuild::declared_type_url(full_name);
        self
    }

    /// Restricts folding to books whose cover carries one of these domains;
    /// [`crate::WILDCARD_DOMAIN`] (`"*"`) among them consumes every domain.
    /// Unset (the default) consumes every domain.
    pub fn for_domains(mut self, domains: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.domains = Some(domains.into_iter().map(Into::into).collect());
        self
    }

    /// Registers the fold thunk for a fully-qualified event type name.
    pub fn on_event(
        mut self,
        full_name: &str,
        thunk: impl for<'a> Fn(&mut P, &Any, &PageContext<'a>) -> Result<(), HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.handlers.insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// Registers the finisher that packs the folded instance into the wire
    /// Projection (its sequence and custom payload). Without it dispatch
    /// returns a default Projection carrying the cover and projector name.
    pub fn finish(
        mut self,
        thunk: impl Fn(&mut P, &pb::EventBook) -> Result<pb::Projection, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.finish = Some(Box::new(thunk));
        self
    }

    /// The rules version the projector registers with: the business
    /// binary's full version, baked in at build. Empty until declared.
    pub fn with_rules_version(mut self, rules_version: impl Into<String>) -> Self {
        self.rules_version = rules_version.into();
        self
    }

    /// The registration hello (types.proto, ComponentDescriptor): kind
    /// PROJECTOR, its declared domains as inputs (`*` when it declares
    /// none), and no state. Refused MISSING_RULES_VERSION without a rules
    /// version.
    pub fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        let inputs: Vec<String> = match self.domains.as_ref() {
            Some(domains) => domains.iter().cloned().collect(),
            None => vec![crate::WILDCARD_DOMAIN.to_string()],
        };
        crate::descriptor::component_descriptor(
            &self.name,
            pb::ComponentKind::Projector,
            inputs,
            &self.rules_version,
            None,
        )
    }

    /// The component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The registered fully-qualified event type names.
    pub fn event_types(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Folds every page of the book into one projection instance, then runs
    /// the finisher (or returns a default Projection). A missing cover fails
    /// with MISSING_EVENT_BOOK_COVER; an undeclared domain folds nothing but
    /// still finishes; an event type with no thunk is skipped.
    pub fn dispatch(&self, events: &pb::EventBook) -> Result<pb::Projection, CodedError> {
        let Some(cover) = events.cover.as_ref() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_EVENT_BOOK_COVER,
                messages::MISSING_EVENT_BOOK_COVER,
                [],
            ));
        };

        let mut projection = (self.factory)();
        let consumed = match self.domains.as_ref() {
            Some(set) => set.contains(crate::WILDCARD_DOMAIN) || set.contains(&cover.domain),
            None => true,
        };

        if let (true, Some(fold)) = (consumed, self.fold.as_ref()) {
            if !events.pages.is_empty() {
                fold(&mut projection, &events.pages, cover).map_err(|failure| {
                    fold_error(
                        PageOrigin::Persisted,
                        &events.pages,
                        &failure,
                        &self.projection_type_url,
                    )
                })?;
            }
        } else if consumed {
            for page in &events.pages {
                let Some(event_any) = crate::page_event(page) else {
                    continue;
                };
                let Some(thunk) = self
                    .handlers
                    .get(crate::type_name_from_url(&event_any.type_url))
                else {
                    // Parity with the Go engine's projectorLogger.Warn.
                    tracing::warn!(
                        projector = %self.name,
                        type_url = %event_any.type_url,
                        "projector received event with no matching handler"
                    );
                    continue;
                };
                let ctx = PageContext {
                    cover: Some(cover),
                    sequence: page.sequence,
                };
                thunk(&mut projection, event_any, &ctx).map_err(|err| {
                    page_fold_error(
                        PageOrigin::Persisted,
                        page,
                        &self.projection_type_url,
                        &handler_error_text(&err),
                    )
                })?;
            }
        }

        match self.finish.as_ref() {
            Some(finish) => finish(&mut projection, events).map_err(map_handler_error),
            None => Ok(pb::Projection {
                cover: Some(cover.clone()),
                projector: self.name.clone(),
                next_sequence: next_sequence_after(events),
                ..Default::default()
            }),
        }
    }
}

/// The projector's position after `events` (types.proto, Projection): the
/// last page's sequence + 1, or the book's next_sequence when it has no
/// pages.
pub fn next_sequence_after(events: &pb::EventBook) -> u64 {
    events
        .pages
        .last()
        .map_or(events.next_sequence, |page| page.sequence + 1)
}

/// A handler error's text, for a failing fold's `fold_message`.
fn handler_error_text(err: &HandlerError) -> String {
    match err {
        HandlerError::Coded(coded) => format!("{}: {}", coded.code, coded.message),
        HandlerError::Other(text) => text.clone(),
    }
}

#[cfg(test)]
#[path = "projector.test.rs"]
mod projector_tests;
