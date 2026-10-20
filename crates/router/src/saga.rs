//! SagaDispatch — the stateless source-event → commands translator.
//!
//! A saga holds NO state and rebuilds nothing: every page of the source book
//! is a fresh trigger. Declared event types emit commands; an undeclared
//! event type is skipped (C-0051), not an error. There is no compensation
//! input. Emitted commands pass through as the handler returned them: the
//! coordinator stamps their identity, causation and correlation.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::PageContext;

/// Translates one source event into commands. Generated thunks unmarshal
/// to the typed event and call the typed business method; the source event's
/// place arrives as a [`PageContext`] (the source book's cover and the
/// event's sequence).
pub type EventFn = Box<
    dyn for<'a> Fn(&Any, &PageContext<'a>) -> Result<Vec<pb::CommandBook>, HandlerError>
        + Send
        + Sync,
>;

/// The dispatch table for one saga component.
pub struct SagaDispatch {
    name: String,
    input_domain: String,
    output_domain: String,
    handlers: HashMap<String, EventFn>,
    rules_version: String,
}

impl SagaDispatch {
    /// An empty saga table translating events of its one source domain,
    /// `input_domain`, into commands for its one destination domain,
    /// `output_domain` (options.proto ComponentOptions; decision 14.24).
    pub fn new(
        name: impl Into<String>,
        input_domain: impl Into<String>,
        output_domain: impl Into<String>,
    ) -> Self {
        SagaDispatch {
            name: name.into(),
            input_domain: input_domain.into(),
            output_domain: output_domain.into(),
            handlers: HashMap::new(),
            rules_version: String::new(),
        }
    }

    /// Registers the translation thunk for a fully-qualified event type; it
    /// receives the source book's cover.
    pub fn on_event(
        self,
        full_name: &str,
        thunk: impl Fn(&Any, Option<&pb::Cover>) -> Result<Vec<pb::CommandBook>, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.on_event_with_context(full_name, move |any, source| thunk(any, source.cover))
    }

    /// Registers the translation thunk for a fully-qualified event type; it
    /// receives the source event's [`PageContext`]: the source book's cover
    /// and the event's sequence (0 when the page carries none).
    pub fn on_event_with_context(
        mut self,
        full_name: &str,
        thunk: impl for<'a> Fn(&Any, &PageContext<'a>) -> Result<Vec<pb::CommandBook>, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.handlers.insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// The rules version the saga registers with: the business binary's
    /// full version, baked in at build. Empty until declared.
    pub fn with_rules_version(mut self, rules_version: impl Into<String>) -> Self {
        self.rules_version = rules_version.into();
        self
    }

    /// The registration hello (types.proto, ComponentDescriptor): kind SAGA,
    /// its input domain as its input, and no state. Refused
    /// MISSING_RULES_VERSION without a rules version.
    pub fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        crate::descriptor::component_descriptor(
            &self.name,
            pb::ComponentKind::Saga,
            [self.input_domain.clone()],
            &self.rules_version,
            None,
        )
    }

    /// The component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The domain whose events this saga consumes.
    pub fn input_domain(&self) -> &str {
        &self.input_domain
    }

    /// The one domain its commands go to.
    pub fn output_domain(&self) -> &str {
        &self.output_domain
    }

    /// The registered fully-qualified event type names.
    pub fn event_types(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// The subscription map: input domain → declared event types.
    pub fn subscriptions(&self) -> HashMap<String, Vec<String>> {
        let mut m = HashMap::new();
        m.insert(self.input_domain.clone(), self.event_types());
        m
    }

    /// Walks EVERY page of the source book: declared event types emit;
    /// undeclared types are skipped. Emitted commands pass through
    /// unstamped, in emission order. A source with no pages is
    /// EMPTY_SAGA_SOURCE.
    pub fn dispatch(&self, source: &pb::EventBook) -> Result<pb::SagaResponse, CodedError> {
        if source.pages.is_empty() {
            return Err(CodedError::invalid_argument(
                codes::EMPTY_SAGA_SOURCE,
                messages::EMPTY_SAGA_SOURCE,
                [],
            ));
        }

        let mut resp = pb::SagaResponse::default();

        for page in &source.pages {
            let Some(event_any) = crate::page_event(page) else {
                continue;
            };
            let Some(thunk) = self
                .handlers
                .get(crate::type_name_from_url(&event_any.type_url))
            else {
                continue; // saga only reacts to declared types (spec C-0051)
            };
            let context = PageContext {
                cover: source.cover.as_ref(),
                sequence: page.sequence,
            };
            let commands = thunk(event_any, &context).map_err(map_handler_error)?;
            if let Some(stray) = commands.iter().find(|c| self.outside_destination(c)) {
                return Err(self.refuse_outside_destination(stray));
            }
            resp.commands.extend(commands);
        }

        Ok(resp)
    }

    fn outside_destination(&self, command: &pb::CommandBook) -> bool {
        command.cover.as_ref().map_or("", |c| c.domain.as_str()) != self.output_domain
    }

    /// A command for any domain but the saga's one destination is a saga
    /// bug (saga.proto SagaResponse; decision 14.24): the whole answer is
    /// refused INTERNAL, carrying no commands, naming both domains.
    fn refuse_outside_destination(&self, stray: &pb::CommandBook) -> CodedError {
        let actual = stray.cover.as_ref().map_or("", |c| c.domain.as_str());
        CodedError::internal(
            codes::SAGA_COMMAND_OUTSIDE_DESTINATION,
            messages::SAGA_COMMAND_OUTSIDE_DESTINATION,
            [
                (
                    extras::EXPECTED_DOMAIN.to_string(),
                    self.output_domain.clone(),
                ),
                (extras::ACTUAL_DOMAIN.to_string(), actual.to_string()),
            ],
        )
    }
}

#[cfg(test)]
#[path = "saga.test.rs"]
mod saga_tests;
