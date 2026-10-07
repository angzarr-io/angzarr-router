//! UpcasterDispatch — ordered event-schema rules for one domain.
//!
//! Every rule whose source type matches the event's current type applies, in
//! registration order, and the output of one is the input of the next (C-0136),
//! so V1 → V2 → V3 composes; the chain stops when no later rule matches the
//! current type (C-0137). Pages keep their order and headers; events of other
//! types, pages without events, and requests for another domain pass through
//! unchanged.

use prost_types::Any;

use crate::error::{map_handler_error, CodedError, HandlerError};
use crate::pb;

/// Transforms one event to its next schema version. Generated thunks unpack
/// the typed old event and pack the typed new one.
pub type UpcastFn = Box<dyn Fn(&Any) -> Result<Any, HandlerError> + Send + Sync>;

/// The rule table for one upcaster component.
pub struct UpcasterDispatch {
    name: String,
    domain: String,
    rules: Vec<(String, UpcastFn)>,
}

impl UpcasterDispatch {
    /// An empty upcaster for `domain`.
    pub fn new(name: impl Into<String>, domain: impl Into<String>) -> Self {
        UpcasterDispatch {
            name: name.into(),
            domain: domain.into(),
            rules: Vec::new(),
        }
    }

    /// Registers a rule transforming events of the fully-qualified
    /// `source_type`.
    pub fn on_event(
        mut self,
        source_type: &str,
        thunk: impl Fn(&Any) -> Result<Any, HandlerError> + Send + Sync + 'static,
    ) -> Self {
        self.rules.push((source_type.to_string(), Box::new(thunk)));
        self
    }

    /// The component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The domain whose events this upcaster transforms.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// The registered source types, in registration order.
    pub fn source_types(&self) -> Vec<&str> {
        self.rules.iter().map(|(t, _)| t.as_str()).collect()
    }

    /// Runs one event through the rule chain.
    pub fn upcast_event(&self, event: &Any) -> Result<Any, CodedError> {
        let mut current = event.clone();
        for (source, rule) in &self.rules {
            if crate::type_name_from_url(&current.type_url) == source {
                current = rule(&current).map_err(map_handler_error)?;
            }
        }
        Ok(current)
    }

    /// `UpcasterService.Upcast`: every page's event through the rule chain.
    pub fn dispatch(&self, req: &pb::UpcastRequest) -> Result<pb::UpcastResponse, CodedError> {
        if req.domain != self.domain {
            return Ok(pb::UpcastResponse {
                events: req.events.clone(),
            });
        }
        let events = req
            .events
            .iter()
            .map(|page| self.upcast_page(page))
            .collect::<Result<_, _>>()?;
        Ok(pb::UpcastResponse { events })
    }

    /// One page with its event (if any) run through the rule chain.
    pub fn upcast_page(&self, page: &pb::EventPage) -> Result<pb::EventPage, CodedError> {
        let mut out = page.clone();
        if let Some(pb::event_page::Payload::Event(event)) = page.payload.as_ref() {
            out.payload = Some(pb::event_page::Payload::Event(self.upcast_event(event)?));
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "upcaster.test.rs"]
mod upcaster_tests;
