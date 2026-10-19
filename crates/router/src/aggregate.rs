//! AggregateDispatch — the command → events table.
//!
//! Envelope and command type validate BEFORE state is rebuilt (an unknown
//! command must report NO_HANDLER_REGISTERED, not whatever the rebuild
//! would surface). There is no compensation input: a Notification-typed
//! payload is just an undeclared command type.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::{pack_message_state, Rebuilder, StatePackFn};

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

    /// Registers the fact handler for a FULLY-QUALIFIED fact (event) type.
    pub fn on_fact(
        mut self,
        fq_fact_type: &str,
        thunk: impl Fn(&Any, &S) -> Result<FactRecord, HandlerError> + Send + Sync + 'static,
    ) -> Self {
        self.facts.insert(fq_fact_type.to_string(), Box::new(thunk));
        self
    }

    /// The component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The aggregate's domain.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// The registered fully-qualified command type names.
    pub fn command_types(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Routes a ContextualCommand: envelope guards with their exact codes,
    /// validate-before-rebuild, rebuild (corrupt persisted event fails the
    /// command), thunk with rebuilt state and CommandContext, fill-only
    /// stamping on the emitted book.
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
