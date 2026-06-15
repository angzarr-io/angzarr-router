//! AggregateDispatch — the command → events table.
//!
//! Envelope and command type validate BEFORE state is rebuilt (an unknown
//! command must report NO_HANDLER_REGISTERED, not whatever the rebuild
//! would surface); rejection routing keys on FULLY-QUALIFIED command type
//! names; multiple compensators all run, in registration order, merging
//! their compensation events into one response.

use std::collections::HashMap;

use prost::Message;
use prost_types::Any;

use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::Rebuilder;

/// Per-dispatch facts the business method may need beyond its typed
/// command and rebuilt state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommandContext {
    /// The aggregate's next event sequence, derived from the prior-events
    /// book the coordinator supplied.
    pub next_sequence: u32,
    /// The "does this aggregate exist" signal — true when the prior-events
    /// book carried any history (pages or snapshot). Exposed because state
    /// factories produce non-default zero states, so business code cannot
    /// infer existence from state.
    pub had_prior_events: bool,
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

/// The dispatch table for one aggregate component.
pub struct AggregateDispatch<S> {
    name: String,
    domain: String,
    rebuilder: Rebuilder<S>,
    handlers: HashMap<String, CommandFn<S>>,
    rejections: HashMap<String, Vec<RejectionFn<S>>>,
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
        }
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

    /// Registers a compensation thunk keyed by the FULLY-QUALIFIED
    /// rejected-command type name. Multiple registrations for the same
    /// command all run, in registration order; their compensation events
    /// merge into one response.
    pub fn on_rejected(
        mut self,
        fq_command_type: &str,
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
            .entry(fq_command_type.to_string())
            .or_default()
            .push(Box::new(thunk));
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
            return self.dispatch_rejection(command_any, req.events.as_ref());
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

    /// Decodes a Notification command page and routes it to the FQ-keyed
    /// compensation entry with rebuilt state.
    fn dispatch_rejection(
        &self,
        command_any: &Any,
        events: Option<&pb::EventBook>,
    ) -> Result<pb::BusinessResponse, CodedError> {
        let notification =
            pb::Notification::decode(command_any.value.as_slice()).map_err(|_| {
                CodedError::invalid_argument(
                    codes::NOTIFICATION_DECODE_FAILED,
                    messages::NOTIFICATION_DECODE_FAILED,
                    [(extras::TYPE_URL.to_string(), command_any.type_url.clone())],
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

        let (_, fq_command) = crate::extract_rejection_key(&rejection);
        let Some(thunks) = self.rejections.get(&fq_command) else {
            // Undeclared: DelegateToFramework (by declaration, not accident).
            return Ok(pb::BusinessResponse::default());
        };
        let (mut state, info) = self.rebuilder.rebuild(events)?;
        let cctx = CommandContext {
            next_sequence: crate::next_sequence(events),
            had_prior_events: info.had_prior_events,
        };
        if let [thunk] = thunks.as_slice() {
            return thunk(&notification, &rejection, &mut state, cctx).map_err(map_handler_error);
        }
        // Fan-out: run every compensator in registration order, merging
        // their compensation events into one response.
        let mut merged = pb::EventBook::default();
        for thunk in thunks {
            let resp =
                thunk(&notification, &rejection, &mut state, cctx).map_err(map_handler_error)?;
            if let Some(pb::business_response::Result::Events(out)) = resp.result {
                merged.pages.extend(out.pages);
            }
        }
        Ok(pb::BusinessResponse {
            result: Some(pb::business_response::Result::Events(merged)),
        })
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
