//! ProcessManagerDispatch — the stateful trigger → commands/process-events
//! table.
//!
//! Transliterated from client-go's `engine.go` ProcessManagerDispatch.Dispatch
//! and `features/process_manager.go`. Unlike the saga, a PM is STATEFUL: it
//! rebuilds its own event-sourced state (reusing [`Rebuilder`]) before each
//! handler. The trigger book is the FULL state of the triggering domain — only
//! the NEWEST page fires, so history never re-triggers. Handlers are keyed by
//! (input domain, fully-qualified event type). A trigger from a domain outside
//! the PM's sources, or an undeclared event type, yields an empty response
//! (spec C-0022), not an error. Emitted commands and process events pass
//! through as the handler returned them: the coordinator stamps the commands'
//! identity and addresses the process events. There is no compensation
//! input.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::{codes, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::Rebuilder;

/// Handles the newest trigger event against rebuilt PM state, returning the
/// full response (process events and commands).
/// Generated thunks unmarshal to the typed event and call the typed business
/// method.
pub type EventFn<S> = Box<
    dyn Fn(
            &Any,
            &mut S,
            Option<&pb::Cover>,
        ) -> Result<pb::ProcessManagerHandleResponse, HandlerError>
        + Send
        + Sync,
>;

/// The dispatch table for one process-manager component.
pub struct ProcessManagerDispatch<S> {
    name: String,
    pm_domain: String,
    targets: Vec<String>,
    rebuilder: Rebuilder<S>,
    /// input domain → fully-qualified event type → thunk.
    handlers: HashMap<String, HashMap<String, EventFn<S>>>,
    rules_version: String,
}

impl<S> ProcessManagerDispatch<S> {
    /// An empty PM table owning `pm_domain`, issuing commands to
    /// `target_domains`, over a Rebuilder for the PM's own event-sourced
    /// state.
    pub fn new(
        name: impl Into<String>,
        pm_domain: impl Into<String>,
        target_domains: impl IntoIterator<Item = impl Into<String>>,
        rebuilder: Rebuilder<S>,
    ) -> Self {
        ProcessManagerDispatch {
            name: name.into(),
            pm_domain: pm_domain.into(),
            targets: target_domains.into_iter().map(Into::into).collect(),
            rebuilder,
            handlers: HashMap::new(),
            rules_version: String::new(),
        }
    }

    /// Registers the thunk for (input domain, fully-qualified event type).
    pub fn on_event(
        mut self,
        input_domain: &str,
        full_name: &str,
        thunk: impl Fn(
                &Any,
                &mut S,
                Option<&pb::Cover>,
            ) -> Result<pb::ProcessManagerHandleResponse, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.handlers
            .entry(input_domain.to_string())
            .or_default()
            .insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// The component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The PM's own domain.
    pub fn pm_domain(&self) -> &str {
        &self.pm_domain
    }

    /// The domains this PM issues commands to.
    pub fn target_domains(&self) -> &[String] {
        &self.targets
    }

    /// The input domains this PM listens to.
    pub fn sources(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// The subscription map: input domain → declared event types.
    pub fn subscriptions(&self) -> HashMap<String, Vec<String>> {
        self.handlers
            .iter()
            .map(|(domain, by_type)| (domain.clone(), by_type.keys().cloned().collect()))
            .collect()
    }

    /// The rules version reported on every response
    /// (ProcessManagerHandleResponse.rules_version; decision 12.6, backlog RV-2): the
    /// business binary's full version string, baked in at build. Empty until
    /// declared.
    pub fn with_rules_version(mut self, rules_version: impl Into<String>) -> Self {
        self.rules_version = rules_version.into();
        self
    }

    /// The registration hello (types.proto, ComponentDescriptor): kind
    /// PROCESS_MANAGER, its source domains as inputs, and the state type its
    /// rebuilder declares. Refused MISSING_RULES_VERSION without a rules
    /// version, MISSING_STATE_TYPE without a state type.
    pub fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        crate::descriptor::component_descriptor(
            &self.name,
            pb::ComponentKind::ProcessManager,
            self.sources(),
            &self.rules_version,
            Some(crate::descriptor::state_descriptor(
                &self.name,
                self.rebuilder.state_type_url(),
                false,
            )?),
        )
    }

    /// The declared rules version.
    pub fn rules_version(&self) -> &str {
        &self.rules_version
    }

    /// Handles a trigger book against rebuilt PM state, setting the declared
    /// rules version on every response, over anything the handler set.
    pub fn dispatch(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        let mut resp = self.handle(req)?;
        resp.rules_version = self.rules_version.clone();
        Ok(resp)
    }

    /// Only the NEWEST page of the trigger fires; a trigger outside the PM's
    /// sources or of an undeclared type yields an empty response (C-0022).
    fn handle(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        let Some(trigger) = req.trigger.as_ref() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_PM_TRIGGER,
                messages::MISSING_PM_TRIGGER,
                [],
            ));
        };
        let Some(last) = trigger.pages.last() else {
            return Err(CodedError::invalid_argument(
                codes::EMPTY_PM_TRIGGER,
                messages::EMPTY_PM_TRIGGER,
                [],
            ));
        };
        let Some(event_any) = crate::page_event(last) else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_PM_EVENT_PAYLOAD,
                messages::MISSING_PM_EVENT_PAYLOAD,
                [],
            ));
        };

        let trigger_domain = trigger
            .cover
            .as_ref()
            .map(|c| c.domain.as_str())
            .unwrap_or("");
        let Some(by_type) = self.handlers.get(trigger_domain) else {
            return Ok(pb::ProcessManagerHandleResponse::default()); // outside sources (C-0022)
        };
        let Some(thunk) = by_type.get(crate::type_name_from_url(&event_any.type_url)) else {
            return Ok(pb::ProcessManagerHandleResponse::default()); // undeclared type
        };

        let (mut state, _info) = self.rebuilder.rebuild(req.process_state.as_ref())?;
        let mut resp =
            thunk(event_any, &mut state, trigger.cover.as_ref()).map_err(map_handler_error)?;
        // The post-reaction state, on every answer the PM handles
        // (process_manager.proto, process_state; decision 14.14): its state
        // after its own process events, which stay payloads only. The
        // coordinator stamps its sequence and rules version.
        let cover = req.process_state.as_ref().and_then(|b| b.cover.as_ref());
        let mut next = crate::next_sequence(req.process_state.as_ref());
        for book in &resp.process_events {
            self.rebuilder
                .apply_emitted(&mut state, &book.pages, cover, next)?;
            next += book.pages.len() as u64;
        }
        resp.process_state = self
            .rebuilder
            .state_image(&state)
            .map(|image| pb::Snapshot {
                state: Some(image),
                ..Default::default()
            });
        Ok(resp)
    }
}

#[cfg(test)]
#[path = "process_manager.test.rs"]
mod process_manager_tests;
