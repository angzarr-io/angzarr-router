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
//! (spec C-0022), not an error. Emitted commands are deferred: the router
//! stamps their `angzarr_deferred` provenance from the trigger (source cover,
//! source_seq, command_index), never an explicit sequence (C-0181). A
//! Notification trigger carrying a RejectionNotification routes to the
//! compensators keyed by the rejected command's type, optionally qualified by
//! the domain it was sent to (ordered, C-0042); their process events merge and
//! the FIRST escalation Notification wins (response field 4). A Compensate
//! payload has no handler on a process manager (undo is aggregate-only) and is
//! answered NO_UNDO_HANDLER (UNIMPLEMENTED), never dropped.

use std::collections::HashMap;

use prost::Message;
use prost_types::Any;

use crate::destinations::Destinations;
use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::{pack_message_state, Rebuilder, StatePackFn};
use crate::NotificationPayload;

/// Handles the newest trigger event against rebuilt PM state, returning the
/// full response (process events, commands, facts, optional escalation).
/// Generated thunks unmarshal to the typed event and call the typed business
/// method.
pub type EventFn<S> = Box<
    dyn Fn(
            &Any,
            &mut S,
            &Destinations,
            Option<&pb::Cover>,
        ) -> Result<pb::ProcessManagerHandleResponse, HandlerError>
        + Send
        + Sync,
>;

/// Compensates a rejected PM-issued command against rebuilt state, returning
/// a full response: process events, commands (deferred like any PM command),
/// facts, and an optional escalation Notification (field 4).
pub type RejectionFn<S> = Box<
    dyn Fn(
            &pb::Notification,
            &pb::RejectionNotification,
            &mut S,
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
    rejections: HashMap<String, Vec<RejectionFn<S>>>,
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
            rejections: HashMap::new(),
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
                &Destinations,
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

    /// Registers a compensation thunk under a `compensates` entry: the
    /// rejected command's fully-qualified type (`"fq.Type"`, any target
    /// domain) or `"domain:fq.Type"` (only rejections of commands sent to that
    /// domain). Repeated registration for one entry appends, preserving order
    /// (C-0042).
    pub fn on_rejected(
        mut self,
        compensates: &str,
        thunk: impl Fn(
                &pb::Notification,
                &pb::RejectionNotification,
                &mut S,
            ) -> Result<pb::ProcessManagerHandleResponse, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.rejections
            .entry(compensates.to_string())
            .or_default()
            .push(Box::new(thunk));
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

    /// Refuses a table whose `compensates` entries list one command type both
    /// unqualified and domain-qualified (AMBIGUOUS_COMPENSATION).
    pub fn validate(&self) -> Result<(), CodedError> {
        crate::validate_compensation_keys(self.rejections.keys().map(String::as_str))
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
            let mut resp = self.dispatch_rejection(event_any, req.process_state.as_ref())?;
            crate::stamp_deferred(
                &mut resp.commands,
                trigger.cover.as_ref(),
                crate::page_sequence(last),
            );
            self.address_process_events(&mut resp, trigger, req.process_state.as_ref());
            return Ok(resp);
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
        let dests = Destinations::new(self.targets.iter().cloned());
        let mut resp = thunk(event_any, &mut state, &dests, trigger.cover.as_ref())
            .map_err(map_handler_error)?;
        crate::stamp_deferred(
            &mut resp.commands,
            trigger.cover.as_ref(),
            crate::page_sequence(last),
        );
        self.address_process_events(&mut resp, trigger, req.process_state.as_ref());
        Ok(resp)
    }

    /// FILL-ONLY addressing of the PM's own stream (C-0477): a process event
    /// the handler left unaddressed takes the PM's domain, the process
    /// state's root and the trigger's correlation id; whatever the handler
    /// set is kept.
    fn address_process_events(
        &self,
        resp: &mut pb::ProcessManagerHandleResponse,
        trigger: &pb::EventBook,
        process_state: Option<&pb::EventBook>,
    ) {
        let state_root = process_state
            .and_then(|s| s.cover.as_ref())
            .and_then(|c| c.root.as_ref());
        let correlation_id = trigger.cover.as_ref().map(|c| c.correlation_id.as_str());
        for book in &mut resp.process_events {
            let cover = book.cover.get_or_insert_with(pb::Cover::default);
            if cover.domain.is_empty() {
                cover.domain = self.pm_domain.clone();
            }
            if cover.root.is_none() {
                cover.root = state_root.cloned();
            }
            if cover.correlation_id.is_empty() {
                if let Some(id) = correlation_id {
                    cover.correlation_id = id.to_string();
                }
            }
        }
    }

    /// Routes a Notification trigger: a RejectionNotification goes to the
    /// compensators matching the rejected command (ordered, C-0042; their
    /// responses merge and the first escalation wins; their commands are
    /// stamped deferred from the notification page), and an undeclared
    /// rejection is the framework's to handle (DelegateToFramework) with an
    /// empty response; a Compensate is NO_UNDO_HANDLER.
    fn dispatch_rejection(
        &self,
        event_any: &Any,
        process_state: Option<&pb::EventBook>,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        let (notification, payload) = crate::decode_notification(event_any)?;
        let rejection = match payload {
            NotificationPayload::Rejection(rejection) => rejection,
            NotificationPayload::Compensate(compensate) => {
                return Err(CodedError::invalid_argument(
                    codes::NO_UNDO_HANDLER,
                    messages::NO_UNDO_HANDLER,
                    [(extras::COMMAND_TYPE.to_string(), compensate.command_type)],
                ));
            }
        };

        let (target_domain, fq_command) = crate::extract_rejection_key(&rejection);
        let Some(thunks) =
            crate::compensation_lookup(&self.rejections, &target_domain, &fq_command)
        else {
            return Ok(pb::ProcessManagerHandleResponse::default()); // DelegateToFramework
        };

        let (mut state, _info) = self.rebuilder.rebuild(process_state)?;
        let mut out = pb::ProcessManagerHandleResponse::default();
        for thunk in thunks {
            let resp = thunk(&notification, &rejection, &mut state).map_err(map_handler_error)?;
            merge_response(&mut out, resp);
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
