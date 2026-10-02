//! AggregateDispatch — the command → events table.
//!
//! Envelope and command type validate BEFORE state is rebuilt (an unknown
//! command must report NO_HANDLER_REGISTERED, not whatever the rebuild
//! would surface). A Notification command page carries one of two payloads:
//! a RejectionNotification routes to the compensators keyed by the rejected
//! command's FULLY-QUALIFIED type, optionally qualified by the domain it was
//! sent to (`compensates`); multiple compensators all run, in registration
//! order, merging their compensation events into one response unless one
//! escalates (the first escalation wins). A Compensate routes to the undo
//! handler registered for its `command_type` (`undoes`); none is
//! NO_UNDO_HANDLER (UNIMPLEMENTED), never a silent drop. Events a
//! compensator or undo handler returns are fill-only stamped like a
//! command's, so they append after prior history.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::{pack_message_state, Rebuilder, StatePackFn};
use crate::NotificationPayload;

/// Per-dispatch facts the business method may need beyond its typed
/// command and rebuilt state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandContext {
    /// The aggregate's next event sequence, derived from the prior-events
    /// book the coordinator supplied.
    pub next_sequence: u32,
    /// The "does this aggregate exist" signal — true when the prior-events
    /// book carried any history (pages or snapshot). Exposed because state
    /// factories produce non-default zero states, so business code cannot
    /// infer existence from state.
    pub had_prior_events: bool,
    /// The cover of the command (or notification delivery) being handled:
    /// the aggregate's own domain and root.
    pub cover: Option<pb::Cover>,
}

/// Handles one command against rebuilt state, emitting an event book (or
/// nothing). Binding/generated thunks unmarshal to the typed command and
/// call the typed business method.
pub type CommandFn<S> = Box<
    dyn Fn(&Any, &mut S, CommandContext) -> Result<Option<pb::EventBook>, HandlerError>
        + Send
        + Sync,
>;

/// Compensates a rejected command against rebuilt state, returning a full
/// BusinessResponse (events, or an escalation Notification).
/// CommandContext supplies next_sequence so compensation events append
/// after prior history.
pub type RejectionFn<S> = Box<
    dyn Fn(
            &pb::Notification,
            &pb::RejectionNotification,
            &mut S,
            CommandContext,
        ) -> Result<pb::BusinessResponse, HandlerError>
        + Send
        + Sync,
>;

/// Undoes a command this aggregate executed, against rebuilt state, given the
/// Compensate notification (the produced event sequences and the reason).
pub type UndoFn<S> = Box<
    dyn Fn(
            &pb::Notification,
            &pb::Compensate,
            &mut S,
            CommandContext,
        ) -> Result<pb::BusinessResponse, HandlerError>
        + Send
        + Sync,
>;

/// What a fact handler records: the fact (as received, or annotated) and the
/// events that flag it, in order. A fact cannot be refused, so there is no
/// way to record nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct FactRecord {
    pub fact: Any,
    pub flags: Vec<Any>,
}

impl From<Any> for FactRecord {
    /// The fact recorded with no flags.
    fn from(fact: Any) -> Self {
        FactRecord {
            fact,
            flags: Vec::new(),
        }
    }
}

/// Records one fact against the rebuilt state (ComponentOptions.facts).
/// Generated thunks unmarshal to the typed fact and pack the typed result.
pub type FactFn<S> = Box<dyn Fn(&Any, &S) -> Result<FactRecord, HandlerError> + Send + Sync>;

/// The dispatch table for one aggregate component.
pub struct AggregateDispatch<S> {
    name: String,
    domain: String,
    rebuilder: Rebuilder<S>,
    handlers: HashMap<String, CommandFn<S>>,
    rejections: HashMap<String, Vec<RejectionFn<S>>>,
    undoes: HashMap<String, UndoFn<S>>,
    facts: HashMap<String, FactFn<S>>,
    state_packer: Option<StatePackFn<S>>,
}

impl<S> AggregateDispatch<S> {
    /// An empty aggregate table over a Rebuilder.
    pub fn new(
        name: impl Into<String>,
        domain: impl Into<String>,
        rebuilder: Rebuilder<S>,
    ) -> Self {
        AggregateDispatch {
            name: name.into(),
            domain: domain.into(),
            rebuilder,
            handlers: HashMap::new(),
            rejections: HashMap::new(),
            undoes: HashMap::new(),
            facts: HashMap::new(),
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

    /// Registers the thunk for a fully-qualified command type name.
    pub fn on_command(
        mut self,
        full_name: &str,
        thunk: impl Fn(&Any, &mut S, CommandContext) -> Result<Option<pb::EventBook>, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.handlers.insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// Registers a compensation thunk under a `compensates` entry: the
    /// rejected command's FULLY-QUALIFIED type (`"fq.Type"`, sent to any
    /// domain) or `"domain:fq.Type"` (only when it was sent to that domain).
    /// Multiple registrations for one entry all run, in registration order;
    /// their compensation events merge into one response, and the first
    /// escalation (Revocation or Notification) wins over events.
    pub fn on_rejected(
        mut self,
        compensates: &str,
        thunk: impl Fn(
                &pb::Notification,
                &pb::RejectionNotification,
                &mut S,
                CommandContext,
            ) -> Result<pb::BusinessResponse, HandlerError>
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

    /// Registers the undo handler for a FULLY-QUALIFIED command type this
    /// aggregate executes (`undoes`): a Compensate whose `command_type` names
    /// it runs this handler. One handler per command type; a later
    /// registration replaces an earlier one.
    pub fn on_undo(
        mut self,
        fq_command_type: &str,
        thunk: impl Fn(
                &pb::Notification,
                &pb::Compensate,
                &mut S,
                CommandContext,
            ) -> Result<pb::BusinessResponse, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.undoes
            .insert(fq_command_type.to_string(), Box::new(thunk));
        self
    }

    /// Registers the fact handler for a FULLY-QUALIFIED fact (event) type.
    pub fn on_fact(
        mut self,
        fq_fact_type: &str,
        thunk: impl Fn(&Any, &S) -> Result<FactRecord, HandlerError> + Send + Sync + 'static,
    ) -> Self {
        self.facts.insert(fq_fact_type.to_string(), Box::new(thunk));
        self
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

    /// The aggregate's domain.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// True when this aggregate declares a handler for the Notification
    /// carried by `notification_any`: a compensation entry matching its
    /// RejectionNotification, or an undo handler for its Compensate's
    /// command type. An undecodable notification is claimed by no one.
    pub fn claims_notification(&self, notification_any: &Any) -> bool {
        match crate::decode_notification(notification_any) {
            Ok((_, NotificationPayload::Rejection(rejection))) => {
                let (domain, fq) = crate::extract_rejection_key(&rejection);
                crate::compensation_lookup(&self.rejections, &domain, &fq).is_some()
            }
            Ok((_, NotificationPayload::Compensate(compensate))) => {
                self.undoes.contains_key(&compensate.command_type)
            }
            Err(_) => false,
        }
    }

    /// The registered fully-qualified command type names.
    pub fn command_types(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Routes a ContextualCommand: envelope guards with their exact codes,
    /// validate-before-rebuild, rebuild (corrupt persisted event fails the
    /// command), thunk with rebuilt state and CommandContext, fill-only
    /// stamping on the emitted book. A Notification command page routes to
    /// the FQ-keyed compensation path instead.
    pub fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::BusinessResponse, CodedError> {
        let Some(command_book) = req.command.as_ref() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_COMMAND_BOOK,
                messages::NO_COMMAND_PAGES,
                [],
            ));
        };
        let Some(first_page) = command_book.pages.first() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_COMMAND_PAGE,
                messages::NO_COMMAND_PAGES,
                [],
            ));
        };
        let command_any = match crate::command_payload(first_page) {
            Some(any) if !any.type_url.is_empty() => any,
            _ => {
                return Err(CodedError::invalid_argument(
                    codes::MISSING_COMMAND_PAYLOAD,
                    messages::NO_COMMAND_PAGES,
                    [],
                ));
            }
        };

        // Exact type-URL match only — suffix matching misroutes user types.
        if crate::is_notification_type_url(&command_any.type_url) {
            let mut resp = self.dispatch_notification(
                command_any,
                req.events.as_ref(),
                command_book.cover.as_ref(),
            )?;
            if let Some(pb::business_response::Result::Events(book)) = resp.result.as_mut() {
                stamp_emitted_book(
                    book,
                    command_book.cover.as_ref(),
                    crate::next_sequence(req.events.as_ref()),
                );
            }
            return Ok(resp);
        }

        let Some(thunk) = self
            .handlers
            .get(crate::type_name_from_url(&command_any.type_url))
        else {
            return Err(CodedError::invalid_argument(
                codes::NO_HANDLER_REGISTERED,
                messages::UNKNOWN_COMMAND,
                [(extras::TYPE_URL.to_string(), command_any.type_url.clone())],
            ));
        };

        let (mut state, info) = self.rebuilder.rebuild(req.events.as_ref())?;
        let next_seq = crate::next_sequence(req.events.as_ref());
        let events = thunk(
            command_any,
            &mut state,
            CommandContext {
                next_sequence: next_seq,
                had_prior_events: info.had_prior_events,
                cover: command_book.cover.clone(),
            },
        )
        .map_err(map_handler_error)?;

        let events = events.map(|mut book| {
            stamp_emitted_book(&mut book, command_book.cover.as_ref(), next_seq);
            book
        });
        Ok(pb::BusinessResponse {
            result: Some(pb::business_response::Result::Events(
                events.unwrap_or_default(),
            )),
        })
    }

    /// Decodes a Notification command page and routes its payload with
    /// rebuilt state: a RejectionNotification to the matching compensators,
    /// a Compensate to the undo handler for its command type.
    fn dispatch_notification(
        &self,
        command_any: &Any,
        events: Option<&pb::EventBook>,
        cover: Option<&pb::Cover>,
    ) -> Result<pb::BusinessResponse, CodedError> {
        let (notification, payload) = crate::decode_notification(command_any)?;
        let rejection = match payload {
            NotificationPayload::Rejection(rejection) => rejection,
            NotificationPayload::Compensate(compensate) => {
                return self.dispatch_undo(&notification, &compensate, events, cover);
            }
        };

        let (target_domain, fq_command) = crate::extract_rejection_key(&rejection);
        let Some(thunks) =
            crate::compensation_lookup(&self.rejections, &target_domain, &fq_command)
        else {
            // Undeclared: DelegateToFramework (by declaration, not accident).
            return Ok(pb::BusinessResponse::default());
        };
        let (mut state, info) = self.rebuilder.rebuild(events)?;
        let cctx = CommandContext {
            next_sequence: crate::next_sequence(events),
            had_prior_events: info.had_prior_events,
            cover: cover.cloned(),
        };
        let mut merged: Option<pb::business_response::Result> = None;
        for thunk in thunks {
            let resp = thunk(&notification, &rejection, &mut state, cctx.clone())
                .map_err(map_handler_error)?;
            merged = merge_compensation(merged, resp.result);
        }
        Ok(pb::BusinessResponse { result: merged })
    }

    /// Runs the undo handler for the Compensate's command type with rebuilt
    /// state; a command type with no undo handler is NO_UNDO_HANDLER.
    fn dispatch_undo(
        &self,
        notification: &pb::Notification,
        compensate: &pb::Compensate,
        events: Option<&pb::EventBook>,
        cover: Option<&pb::Cover>,
    ) -> Result<pb::BusinessResponse, CodedError> {
        let Some(thunk) = self.undoes.get(&compensate.command_type) else {
            return Err(CodedError::invalid_argument(
                codes::NO_UNDO_HANDLER,
                messages::NO_UNDO_HANDLER,
                [(
                    extras::COMMAND_TYPE.to_string(),
                    compensate.command_type.clone(),
                )],
            ));
        };
        let (mut state, info) = self.rebuilder.rebuild(events)?;
        let cctx = CommandContext {
            next_sequence: crate::next_sequence(events),
            had_prior_events: info.had_prior_events,
            cover: cover.cloned(),
        };
        thunk(notification, compensate, &mut state, cctx).map_err(map_handler_error)
    }

    /// `CommandHandlerService.HandleFact`: every fact type must have a fact
    /// handler (ComponentOptions.facts) — an undeclared one refuses the whole
    /// request with NO_FACT_HANDLER (INVALID_ARGUMENT) before any handler runs,
    /// so nothing is recorded. Then state rebuilds from the prior events and
    /// the facts are walked in order: each fact's handler returns the fact to
    /// record (it may annotate it, never refuse it) followed by the events
    /// that flag it, and every recorded event folds into the state the next
    /// fact sees. The facts' cover and each fact's header are kept; a flag
    /// carries no header and its fact's `created_at`. The coordinator assigns
    /// real sequences. A page with no event is recorded unchanged.
    pub fn handle_fact(&self, req: &pb::FactRequest) -> Result<pb::EventBook, CodedError> {
        let pages = req.facts.as_ref().map_or(&[][..], |f| f.pages.as_slice());
        for event in pages.iter().filter_map(crate::page_event) {
            if !self
                .facts
                .contains_key(crate::type_name_from_url(&event.type_url))
            {
                return Err(CodedError::invalid_argument(
                    codes::NO_FACT_HANDLER,
                    messages::NO_FACT_HANDLER,
                    [(extras::TYPE_URL.to_string(), event.type_url.clone())],
                ));
            }
        }
        let (mut state, _) = self.rebuilder.rebuild(req.prior_events.as_ref())?;
        let Some(facts) = req.facts.as_ref() else {
            return Ok(pb::EventBook::default());
        };
        let cover = facts.cover.as_ref();
        let mut out = pb::EventBook {
            cover: facts.cover.clone(),
            ..Default::default()
        };
        for page in &facts.pages {
            let Some(event) = crate::page_event(page) else {
                out.pages.push(page.clone());
                continue;
            };
            let thunk = &self.facts[crate::type_name_from_url(&event.type_url)];
            let record = thunk(event, &state).map_err(map_handler_error)?;
            let recorded = pb::EventPage {
                payload: Some(pb::event_page::Payload::Event(record.fact)),
                ..page.clone()
            };
            self.rebuilder.apply_page(&mut state, &recorded, cover)?;
            out.pages.push(recorded);
            for flag in record.flags {
                let flagged = pb::EventPage {
                    header: None,
                    created_at: page.created_at,
                    payload: Some(pb::event_page::Payload::Event(flag)),
                };
                self.rebuilder.apply_page(&mut state, &flagged, cover)?;
                out.pages.push(flagged);
            }
        }
        Ok(out)
    }

    /// `CommandHandlerService.Replay`: the state after folding the base
    /// snapshot (when present) and then the events, in order. The caller packs
    /// the state into the response's `Any`.
    pub fn replay(&self, req: &pb::ReplayRequest) -> Result<S, CodedError> {
        self.rebuilder.replay(req)
    }

    /// [`Self::replay`] packed by the state packer; without one the aggregate
    /// does not support Replay (NO_HANDLER_REGISTERED).
    pub fn packed_replay(&self, req: &pb::ReplayRequest) -> Result<pb::ReplayResponse, CodedError> {
        self.rebuilder
            .packed_replay(self.state_packer.as_ref(), &self.domain, req)
    }
}

/// Folds one compensator's result into the fan-out result. Compensators run in
/// registration order (C-0042); their compensation events concatenate under the
/// first events book's cover, and the first escalation (a Revocation or a
/// Notification) wins over events and over later escalations. A compensator
/// returning nothing contributes nothing. One compensator's result is
/// therefore returned unchanged.
pub(crate) fn merge_compensation(
    acc: Option<pb::business_response::Result>,
    next: Option<pb::business_response::Result>,
) -> Option<pb::business_response::Result> {
    use pb::business_response::Result as R;
    match (acc, next) {
        (None, next) => next,
        (acc, None) => acc,
        (Some(R::Events(mut book)), Some(R::Events(more))) => {
            book.pages.extend(more.pages);
            Some(R::Events(book))
        }
        (Some(R::Events(_)), escalation) => escalation,
        (escalation, _) => escalation,
    }
}

/// Applies the dispatch path's FILL-ONLY stamps to an emitted EventBook:
///   - the command cover's ext propagates onto the book's cover so child
///     aggregates carry their parent linkage — never overriding an ext
///     the handler set itself;
///   - pages without headers receive consecutive sequences from the
///     aggregate's next sequence — explicit headers are preserved.
fn stamp_emitted_book(events: &mut pb::EventBook, cmd_cover: Option<&pb::Cover>, next_seq: u32) {
    if let Some(ext) = cmd_cover.and_then(|c| c.ext.as_ref()) {
        let cover = events.cover.get_or_insert_with(pb::Cover::default);
        if cover.ext.is_none() {
            cover.ext = Some(ext.clone());
        }
    }
    let mut seq = next_seq;
    for page in &mut events.pages {
        if page.header.is_none() {
            page.header = Some(pb::PageHeader {
                sequence_type: Some(pb::page_header::SequenceType::Sequence(seq)),
                ..Default::default()
            });
        }
        seq += 1;
    }
}

#[cfg(test)]
#[path = "aggregate.test.rs"]
mod aggregate_tests;
