//! SagaDispatch — the stateless source-event → commands translator.
//!
//! Transliterated from client-go's `engine.go` SagaDispatch.Dispatch +
//! `features/saga.go`. A saga holds NO state and rebuilds nothing: every
//! page of the source book is a fresh trigger. Declared event types emit
//! commands (the coordinator stamps their sequences and correlation); an
//! undeclared event type is silently skipped
//! (DelegateToFramework), not an error. There is no compensation input.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::{codes, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;

/// Translates one source event (with the source book's cover) into
/// commands. Generated thunks unmarshal to the typed event and call the
/// typed business method.
pub type EventFn = Box<
    dyn Fn(&Any, Option<&pb::Cover>) -> Result<Vec<pb::CommandBook>, HandlerError> + Send + Sync,
>;

/// The dispatch table for one saga component.
pub struct SagaDispatch {
    name: String,
    input_domain: String,
    targets: Vec<String>,
    handlers: HashMap<String, EventFn>,
}

impl SagaDispatch {
    /// An empty saga table translating `input_domain` events into commands
    /// for `target_domains`.
    pub fn new(
        name: impl Into<String>,
        input_domain: impl Into<String>,
        target_domains: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        SagaDispatch {
            name: name.into(),
            input_domain: input_domain.into(),
            targets: target_domains.into_iter().map(Into::into).collect(),
            handlers: HashMap::new(),
        }
    }

    /// Registers the translation thunk for a fully-qualified event type.
    pub fn on_event(
        mut self,
        full_name: &str,
        thunk: impl Fn(&Any, Option<&pb::Cover>) -> Result<Vec<pb::CommandBook>, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.handlers.insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// The component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The domain whose events this saga consumes.
    pub fn input_domain(&self) -> &str {
        &self.input_domain
    }

    /// The domains this saga issues commands to.
    pub fn target_domains(&self) -> &[String] {
        &self.targets
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
    /// unstamped (the coordinator stamps correlation). A nil source is
    /// MISSING_SAGA_SOURCE; a source with no pages is EMPTY_SAGA_SOURCE.
    pub fn dispatch(&self, req: &pb::SagaHandleRequest) -> Result<pb::SagaResponse, CodedError> {
        let Some(source) = req.source.as_ref() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_SAGA_SOURCE,
                messages::MISSING_SAGA_SOURCE,
                [],
            ));
        };
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
            let commands = thunk(event_any, source.cover.as_ref()).map_err(map_handler_error)?;
            resp.commands.extend(commands);
        }

        Ok(resp)
    }
}

#[cfg(test)]
#[path = "saga.test.rs"]
mod saga_tests;
