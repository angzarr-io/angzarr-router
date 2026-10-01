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
//! (spec C-0022), not an error. A Notification trigger routes to the FQ-keyed
//! compensators (ordered, C-0042); their process events merge and the FIRST
//! escalation Notification wins (response field 4).

use std::collections::HashMap;

use prost::Message;
use prost_types::Any;

use crate::destinations::Destinations;
use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::Rebuilder;

/// Handles the newest trigger event against rebuilt PM state, returning the
/// full response (process events, commands, facts, optional escalation).
/// Generated thunks unmarshal to the typed event and call the typed business
/// method.
pub type EventFn<S> = Box<
    dyn Fn(&Any, &mut S, &Destinations) -> Result<pb::ProcessManagerHandleResponse, HandlerError>
        + Send
        + Sync,
>;

/// Compensates a rejected PM-issued command against rebuilt state, returning
/// process events and an optional escalation Notification (rides field 4 of
/// the response).
pub type RejectionFn<S> = Box<
    dyn Fn(
            &pb::Notification,
            &pb::RejectionNotification,
            &mut S,
        ) -> Result<(Vec<pb::EventBook>, Option<pb::Notification>), HandlerError>
        + Send
        + Sync,
>;

/// The dispatch table for one process-manager component.
pub struct ProcessManagerDispatch<S> {
    name: String,
    pm_domain: String,
    rebuilder: Rebuilder<S>,
    /// input domain → fully-qualified event type → thunk.
    handlers: HashMap<String, HashMap<String, EventFn<S>>>,
    rejections: HashMap<String, Vec<RejectionFn<S>>>,
}

impl<S> ProcessManagerDispatch<S> {
    /// An empty PM table over a Rebuilder for the PM's own event-sourced
    /// state.
    pub fn new(
        name: impl Into<String>,
        pm_domain: impl Into<String>,
        rebuilder: Rebuilder<S>,
    ) -> Self {
        ProcessManagerDispatch {
            name: name.into(),
            pm_domain: pm_domain.into(),
            rebuilder,
            handlers: HashMap::new(),
            rejections: HashMap::new(),
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
                &Destinations,
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

    /// Registers a compensation thunk keyed by the fully-qualified rejected
    /// command type. Repeated registration for one command appends, preserving
    /// order (C-0042).
    pub fn on_rejected(
        mut self,
        fq_command_type: &str,
        thunk: impl Fn(
                &pb::Notification,
                &pb::RejectionNotification,
                &mut S,
            ) -> Result<(Vec<pb::EventBook>, Option<pb::Notification>), HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.rejections
            .entry(fq_command_type.to_string())
            .or_default()
            .push(Box::new(thunk));
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
    /// undeclared type yields an empty response (C-0022). A Notification page
    /// routes to the compensation path.
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

        // Exact type-URL match only — suffix matching misroutes user types.
        if crate::is_notification_type_url(&event_any.type_url) {
            return self.dispatch_rejection(event_any, req.process_state.as_ref());
        }

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
        let dests = Destinations::new(req.destination_sequences.clone());
        thunk(event_any, &mut state, &dests).map_err(map_handler_error)
    }

    /// Routes a Notification trigger to the FQ-keyed compensators (ordered,
    /// C-0042); process events merge and the first escalation wins. An
    /// undeclared rejection is the framework's to handle (DelegateToFramework)
    /// and yields an empty response.
    fn dispatch_rejection(
        &self,
        event_any: &Any,
        process_state: Option<&pb::EventBook>,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
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
            return Ok(pb::ProcessManagerHandleResponse::default()); // DelegateToFramework
        };

        let (mut state, _info) = self.rebuilder.rebuild(process_state)?;
        let mut out = pb::ProcessManagerHandleResponse::default();
        for thunk in thunks {
            let (process_events, escalation) =
                thunk(&notification, &rejection, &mut state).map_err(map_handler_error)?;
            out.process_events.extend(process_events);
            if out.notification.is_none() {
                out.notification = escalation;
            }
        }
        Ok(out)
    }
}

/// The routing view of one process-manager component: its identity and the
/// input domains it consumes. Implemented by every [`ProcessManagerDispatch`]
/// regardless of its state type, so co-resident PMs with different state types
/// route through one selection.
pub trait ProcessManagerRoute {
    /// The registered component name (matched against
    /// `AngzarrDeferredSequence.source_component`).
    fn name(&self) -> &str;
    /// The PM's own domain (matched against the process-state cover and the
    /// rejection's source cover).
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
/// A process-state book belongs to exactly one PM, and a rejection belongs to
/// the PM that issued the rejected command, so identity routes first:
///
/// - Rejection Notification trigger: the PM named by the rejected command's
///   `angzarr_deferred.source_component`; else the PM owning its
///   `angzarr_deferred.source` domain; else the PM owning the trigger's cover
///   domain (where the coordinator delivers a PM's rejections). An unaddressed
///   rejection reaches a sole registered PM and otherwise no PM — it never
///   fans out across every subscriber.
/// - Event trigger: the PM owning the process-state cover domain; when the
///   state carries no identity (a new workflow) or names no registered PM,
///   every PM consuming the trigger's domain.
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
    let Some(newest) = trigger.pages.last() else {
        return Vec::new();
    };
    let trigger_domain = trigger.cover.as_ref().map_or("", |c| c.domain.as_str());
    let matching = |pred: &dyn Fn(&dyn ProcessManagerRoute) -> bool| -> Vec<usize> {
        pms.iter()
            .enumerate()
            .filter(|(_, pm)| pred(**pm))
            .map(|(i, _)| i)
            .collect()
    };

    let notification =
        crate::page_event(newest).filter(|any| crate::is_notification_type_url(&any.type_url));
    if let Some(any) = notification {
        let (component, source_domain) = rejection_issuer(any);
        if !component.is_empty() {
            let named = matching(&|pm| pm.name() == component);
            if !named.is_empty() {
                return named;
            }
        }
        for domain in [source_domain.as_str(), trigger_domain] {
            if domain.is_empty() {
                continue;
            }
            let owners = matching(&|pm| pm.pm_domain() == domain);
            if !owners.is_empty() {
                return owners;
            }
        }
        return if pms.len() == 1 { vec![0] } else { Vec::new() };
    }

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

/// The issuing component name and source domain recorded on a rejection
/// Notification's rejected command (`angzarr_deferred`), or empty strings when
/// the notification carries no decodable provenance.
fn rejection_issuer(notification_any: &Any) -> (String, String) {
    let Ok(notification) = pb::Notification::decode(notification_any.value.as_slice()) else {
        return (String::new(), String::new());
    };
    let Some(rejection) = notification
        .payload
        .as_ref()
        .and_then(|p| pb::RejectionNotification::decode(p.value.as_slice()).ok())
    else {
        return (String::new(), String::new());
    };
    let deferred = rejection
        .rejected_command
        .as_ref()
        .and_then(|cmd| cmd.pages.first())
        .and_then(|page| page.header.as_ref())
        .and_then(|h| match h.sequence_type.as_ref() {
            Some(pb::page_header::SequenceType::AngzarrDeferred(d)) => Some(d),
            _ => None,
        });
    match deferred {
        Some(d) => (
            d.source_component.clone(),
            d.source
                .as_ref()
                .map(|c| c.domain.clone())
                .unwrap_or_default(),
        ),
        None => (String::new(), String::new()),
    }
}

/// Folds one co-resident PM's response into the merged response: process
/// events, commands and facts concatenate in dispatch order; the first
/// escalation Notification wins.
pub fn merge_response(
    acc: &mut pb::ProcessManagerHandleResponse,
    resp: pb::ProcessManagerHandleResponse,
) {
    acc.process_events.extend(resp.process_events);
    acc.commands.extend(resp.commands);
    acc.facts.extend(resp.facts);
    if acc.notification.is_none() {
        acc.notification = resp.notification;
    }
}

#[cfg(test)]
#[path = "process_manager.test.rs"]
mod process_manager_tests;
