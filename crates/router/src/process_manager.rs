//! ProcessManagerDispatch — the stateful trigger → commands/process-events/
//! facts table.
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
//! input: a Notification-typed trigger is just an
//! undeclared event type.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::{codes, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::{pack_message_state, Rebuilder, StatePackFn};

/// Handles the newest trigger event against rebuilt PM state, returning the
/// full response (process events, commands, facts, optional escalation).
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
    state_packer: Option<StatePackFn<S>>,
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
            state_packer: None,
        }
    }

    /// Supports Replay through the router: replayed state is packed by
    /// `packer` into the response's `Any`.
    pub fn with_state_packer(
        mut self,
        packer: impl Fn(&S) -> Result<Any, HandlerError> + Send + Sync + 'static,
    ) -> Self {
        self.state_packer = Some(Box::new(packer));
        self
    }

    /// Supports Replay through the router for a protobuf-message state,
    /// packed under its fully-qualified name.
    pub fn with_message_state(self) -> Self
    where
        S: prost::Message + prost::Name + 'static,
    {
        self.with_state_packer(pack_message_state::<S>)
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

    /// The PM's state after folding the base snapshot (when present) and
    /// then the events, in order — the process-manager counterpart of the
    /// aggregate's `Replay`.
    pub fn replay(&self, req: &pb::ReplayRequest) -> Result<S, CodedError> {
        self.rebuilder.replay(req)
    }

    /// [`Self::replay`] packed by the state packer; without one the process
    /// manager does not support Replay (NO_HANDLER_REGISTERED).
    pub fn packed_replay(&self, req: &pb::ReplayRequest) -> Result<pb::ReplayResponse, CodedError> {
        self.rebuilder
            .packed_replay(self.state_packer.as_ref(), &self.pm_domain, req)
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

    /// Handles a trigger book against rebuilt PM state. Only the NEWEST page
    /// of the trigger fires; a trigger outside the PM's sources or of an
    /// undeclared type yields an empty response (C-0022).
    pub fn dispatch(
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
        thunk(event_any, &mut state, trigger.cover.as_ref()).map_err(map_handler_error)
    }
}

/// The routing view of one process-manager component: its identity and the
/// input domains it consumes. Implemented by every [`ProcessManagerDispatch`]
/// regardless of its state type, so co-resident PMs with different state types
/// route through one selection.
pub trait ProcessManagerRoute {
    /// The registered component name.
    fn name(&self) -> &str;
    /// The PM's own domain (matched against the process-state cover).
    fn pm_domain(&self) -> &str;
    /// True when the PM declares a handler for some event of `domain`.
    fn consumes(&self, domain: &str) -> bool;
}

impl<S> ProcessManagerRoute for ProcessManagerDispatch<S> {
    fn name(&self) -> &str {
        &self.name
    }

    fn pm_domain(&self) -> &str {
        &self.pm_domain
    }

    fn consumes(&self, domain: &str) -> bool {
        self.handlers.contains_key(domain)
    }
}

/// Selects which of several co-resident process managers a request is
/// addressed to, as indices into `pms` in registration order.
///
/// A process-state book belongs to exactly one PM, so identity routes first:
/// the PM owning the process-state cover domain; when the state carries no
/// identity (a new workflow) or names no registered PM, every PM consuming the
/// trigger's domain.
///
/// A missing or page-less trigger selects nothing; the caller reports those
/// shapes before routing.
pub fn select_process_managers(
    pms: &[&dyn ProcessManagerRoute],
    req: &pb::ProcessManagerHandleRequest,
) -> Vec<usize> {
    let Some(trigger) = req.trigger.as_ref() else {
        return Vec::new();
    };
    if trigger.pages.is_empty() {
        return Vec::new();
    }
    let trigger_domain = trigger.cover.as_ref().map_or("", |c| c.domain.as_str());
    let matching = |pred: &dyn Fn(&dyn ProcessManagerRoute) -> bool| -> Vec<usize> {
        pms.iter()
            .enumerate()
            .filter(|(_, pm)| pred(**pm))
            .map(|(i, _)| i)
            .collect()
    };

    let state_domain = req
        .process_state
        .as_ref()
        .and_then(|s| s.cover.as_ref())
        .map_or("", |c| c.domain.as_str());
    if !state_domain.is_empty() {
        let owners = matching(&|pm| pm.pm_domain() == state_domain);
        if !owners.is_empty() {
            return owners;
        }
    }
    matching(&|pm| pm.consumes(trigger_domain))
}

/// Folds one co-resident PM's response into the merged response: process
/// events, commands and facts concatenate in dispatch order.
pub fn merge_response(
    acc: &mut pb::ProcessManagerHandleResponse,
    resp: pb::ProcessManagerHandleResponse,
) {
    acc.process_events.extend(resp.process_events);
    acc.commands.extend(resp.commands);
    acc.facts.extend(resp.facts);
}

#[cfg(test)]
#[path = "process_manager.test.rs"]
mod process_manager_tests;
