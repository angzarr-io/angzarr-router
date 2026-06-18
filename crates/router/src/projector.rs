//! ProjectorDispatch — the events → projection table.
//!
//! One projection instance per delivery folds every declared event page in
//! order (the instance is reused across pages — spec C-0086), then an
//! optional Finish packs it into the wire Projection. Unlike the aggregate
//! there is no prior-state rebuild, no snapshot, and no rejection path: an
//! event with no registered handler is skipped (a registered `on_unknown`
//! sees its type URL); events whose book cover carries an undeclared domain
//! never fold, though Finish still runs.

use std::collections::{HashMap, HashSet};

use prost_types::Any;

use crate::error::{codes, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;

/// Folds one delivered event page into the rebuilding projection. Generated
/// thunks unmarshal to the typed event and call the typed business method.
pub type EventFn<P> = Box<dyn Fn(&mut P, &Any) -> Result<(), HandlerError> + Send + Sync>;

/// Packs the folded projection instance into the wire Projection. When
/// absent, dispatch returns a default Projection (cover + projector name).
pub type FinishFn<P> =
    Box<dyn Fn(&mut P, &pb::EventBook) -> Result<pb::Projection, HandlerError> + Send + Sync>;

/// Observes the type URL of an event that matched no fold thunk.
pub type UnknownFn = Box<dyn Fn(&str) + Send + Sync>;

/// The dispatch table for one projector component.
pub struct ProjectorDispatch<P> {
    name: String,
    factory: Box<dyn Fn() -> P + Send + Sync>,
    domains: Option<HashSet<String>>,
    handlers: HashMap<String, EventFn<P>>,
    unknown: Option<UnknownFn>,
    finish: Option<FinishFn<P>>,
}

impl<P> ProjectorDispatch<P> {
    /// An empty projector table over a fresh-projection factory.
    pub fn new(name: impl Into<String>, factory: impl Fn() -> P + Send + Sync + 'static) -> Self {
        ProjectorDispatch {
            name: name.into(),
            factory: Box::new(factory),
            domains: None,
            handlers: HashMap::new(),
            unknown: None,
            finish: None,
        }
    }

    /// Restricts folding to books whose cover carries one of these domains.
    /// Unset (the default) consumes every domain.
    pub fn for_domains(mut self, domains: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.domains = Some(domains.into_iter().map(Into::into).collect());
        self
    }

    /// Registers the fold thunk for a fully-qualified event type name.
    pub fn on_event(
        mut self,
        full_name: &str,
        thunk: impl Fn(&mut P, &Any) -> Result<(), HandlerError> + Send + Sync + 'static,
    ) -> Self {
        self.handlers.insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// Registers a catch-all for events with no fold thunk. Without one such
    /// events are skipped and a warn is logged (parity with the Go engine's
    /// projectorLogger.Warn); a binding that wants to react registers here.
    pub fn on_unknown(mut self, thunk: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.unknown = Some(Box::new(thunk));
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
            Some(set) => set.contains(&cover.domain),
            None => true,
        };

        if consumed {
            for page in &events.pages {
                let Some(event_any) = crate::page_event(page) else {
                    continue;
                };
                let Some(thunk) = self
                    .handlers
                    .get(crate::type_name_from_url(&event_any.type_url))
                else {
                    match self.unknown.as_ref() {
                        Some(unknown) => unknown(&event_any.type_url),
                        // Parity with the Go engine's projectorLogger.Warn.
                        None => tracing::warn!(
                            projector = %self.name,
                            type_url = %event_any.type_url,
                            "projector received event with no matching handler"
                        ),
                    }
                    continue;
                };
                thunk(&mut projection, event_any).map_err(map_handler_error)?;
            }
        }

        match self.finish.as_ref() {
            Some(finish) => finish(&mut projection, events).map_err(map_handler_error),
            None => Ok(pb::Projection {
                cover: Some(cover.clone()),
                projector: self.name.clone(),
                ..Default::default()
            }),
        }
    }
}

#[cfg(test)]
#[path = "projector.test.rs"]
mod projector_tests;
