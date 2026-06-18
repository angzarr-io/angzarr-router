//! SagaDispatch — the stateless source-event → commands+events translator.
//!
//! Transliterated from client-go's `engine.go` SagaDispatch.Dispatch +
//! `features/saga.go`. A saga holds NO state and rebuilds nothing: every
//! page of the source book is a fresh trigger. Declared event types emit
//! commands (stamped from the coordinator-supplied Destinations) and/or
//! injected fact events; a Notification page routes to the FQ-keyed
//! compensation thunks (ordered, C-0042); an undeclared event type or
//! undeclared rejection is silently skipped (DelegateToFramework), not an
//! error. Emitted commands inherit the source correlation id FILL-ONLY.

use std::collections::HashMap;

use prost::Message;
use prost_types::Any;

use crate::destinations::Destinations;
use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;

/// Translates one source event into commands and/or injected fact events.
/// Generated thunks unmarshal to the typed event, call the typed business
/// method, and stamp emitted commands via the supplied Destinations.
pub type EventFn = Box<
    dyn Fn(&Any, &Destinations) -> Result<(Vec<pb::CommandBook>, Vec<pb::EventBook>), HandlerError>
        + Send
        + Sync,
>;

/// Compensates a rejected command, returning fact events to inject. Keyed
/// by fully-qualified command type; multiple thunks for one command run in
/// registration order (C-0042).
pub type RejectionFn = Box<
    dyn Fn(
            &pb::Notification,
            &pb::RejectionNotification,
        ) -> Result<Vec<pb::EventBook>, HandlerError>
        + Send
        + Sync,
>;

/// The dispatch table for one saga component.
pub struct SagaDispatch {
    name: String,
    input_domain: String,
    targets: Vec<String>,
    handlers: HashMap<String, EventFn>,
    rejections: HashMap<String, Vec<RejectionFn>>,
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
            rejections: HashMap::new(),
        }
    }

    /// Registers the translation thunk for a fully-qualified event type.
    pub fn on_event(
        mut self,
        full_name: &str,
        thunk: impl Fn(
                &Any,
                &Destinations,
            ) -> Result<(Vec<pb::CommandBook>, Vec<pb::EventBook>), HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.handlers.insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// Registers a compensation thunk for a fully-qualified command type.
    /// Repeated registration for one command appends, preserving order
    /// (C-0042).
    pub fn on_rejected(
        mut self,
        fq_command: &str,
        thunk: impl Fn(
                &pb::Notification,
                &pb::RejectionNotification,
            ) -> Result<Vec<pb::EventBook>, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.rejections
            .entry(fq_command.to_string())
            .or_default()
            .push(Box::new(thunk));
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

    /// Walks EVERY page of the source book: notification pages route to the
    /// FQ-keyed compensation thunks; declared event types emit; undeclared
    /// types are skipped. Emitted commands inherit the source correlation id
    /// fill-only. A nil source is MISSING_SAGA_SOURCE; a source with no
    /// pages is EMPTY_SAGA_SOURCE.
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

        let dests = Destinations::new(req.destination_sequences.clone());
        let mut resp = pb::SagaResponse::default();

        for page in &source.pages {
            let Some(event_any) = crate::page_event(page) else {
                continue;
            };

            // Exact type-URL match only — suffix matching misroutes user types.
            if crate::is_notification_type_url(&event_any.type_url) {
                resp.events.extend(self.dispatch_rejection(event_any)?);
                continue;
            }

            let Some(thunk) = self
                .handlers
                .get(crate::type_name_from_url(&event_any.type_url))
            else {
                continue; // saga only reacts to declared types (spec C-0051)
            };
            let (commands, events) = thunk(event_any, &dests).map_err(map_handler_error)?;
            resp.commands.extend(commands);
            resp.events.extend(events);
        }

        // FILL-ONLY correlation propagation: emitted commands inherit the
        // source book's correlation id unless the handler stamped one — the
        // saga sibling of the aggregate's stampEmittedBook.
        if let Some(corr) = source
            .cover
            .as_ref()
            .map(|c| c.correlation_id.as_str())
            .filter(|c| !c.is_empty())
        {
            for cmd in &mut resp.commands {
                let cover = cmd.cover.get_or_insert_with(pb::Cover::default);
                if cover.correlation_id.is_empty() {
                    cover.correlation_id = corr.to_string();
                }
            }
        }
        Ok(resp)
    }

    /// Decodes a Notification page and routes it to the FQ-keyed
    /// compensation thunks (ordered, C-0042). An undeclared rejection is the
    /// framework's to handle (DelegateToFramework) and yields no events.
    fn dispatch_rejection(&self, event_any: &Any) -> Result<Vec<pb::EventBook>, CodedError> {
        let notification = pb::Notification::decode(event_any.value.as_slice()).map_err(|_| {
            CodedError::invalid_argument(
                codes::NOTIFICATION_DECODE_FAILED,
                messages::NOTIFICATION_DECODE_FAILED,
                [(extras::TYPE_URL.to_string(), event_any.type_url.clone())],
            )
        })?;

        let rejection = match notification.payload.as_ref() {
            Some(payload) => {
                pb::RejectionNotification::decode(payload.value.as_slice()).map_err(|_| {
                    CodedError::invalid_argument(
                        codes::REJECTION_NOTIFICATION_DECODE_FAILED,
                        messages::REJECTION_NOTIFICATION_DECODE_FAILED,
                        [],
                    )
                })?
            }
            None => pb::RejectionNotification::default(),
        };

        let (_domain, fq_command) = crate::extract_rejection_key(&rejection);
        let Some(thunks) = self.rejections.get(&fq_command) else {
            return Ok(Vec::new()); // undeclared: DelegateToFramework
        };

        let mut events = Vec::new();
        for thunk in thunks {
            events.extend(thunk(&notification, &rejection).map_err(map_handler_error)?);
        }
        Ok(events)
    }
}

#[cfg(test)]
#[path = "saga.test.rs"]
mod saga_tests;
