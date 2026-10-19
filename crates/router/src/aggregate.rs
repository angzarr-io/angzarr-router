//! AggregateDispatch — the command → events table.
//!
//! `CommandHandlerService.Handle` is the only input path: the body arm of the
//! ContextualCommand's page picks the pipeline (Command → a command handler,
//! Fact → a fact handler) and the payload's type URL picks the handler. The
//! envelope, the arm and the type validate BEFORE state is rebuilt (an
//! unknown command must report NO_HANDLER_REGISTERED, not whatever the
//! rebuild would surface). There is no compensation input.

use std::collections::HashMap;

use prost_types::Any;

use crate::error::{codes, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::Rebuilder;

/// Per-dispatch facts the business method may need beyond its typed
/// command and rebuilt state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandContext {
    /// The aggregate's next event sequence, derived from the prior-events
    /// book the coordinator supplied.
    pub next_sequence: u64,
    /// The "does this aggregate exist" signal — true when the prior-events
    /// book carried any history (pages or snapshot). Exposed because state
    /// factories produce non-default zero states, so business code cannot
    /// infer existence from state.
    pub had_prior_events: bool,
    /// The cover of the command being handled: the aggregate's own domain
    /// and root.
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
/// events that flag it, in order — the fact command's events. A fact cannot
/// be refused, so there is no way to record nothing.
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

/// Records one fact (a command page's Fact arm) against the rebuilt state.
/// Generated thunks unmarshal to the typed fact and pack the typed result.
pub type FactFn<S> = Box<dyn Fn(&Any, &S) -> Result<FactRecord, HandlerError> + Send + Sync>;

/// The dispatch table for one aggregate component.
pub struct AggregateDispatch<S> {
    name: String,
    domain: String,
    rebuilder: Rebuilder<S>,
    handlers: HashMap<String, CommandFn<S>>,
    facts: HashMap<String, FactFn<S>>,
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

    /// Registers the fact handler for a FULLY-QUALIFIED fact type: a page
    /// whose Fact arm carries it runs this handler.
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

    /// The registered fully-qualified fact type names.
    pub fn fact_types(&self) -> Vec<String> {
        self.facts.keys().cloned().collect()
    }

    /// `CommandHandlerService.Handle`: envelope guards with their exact
    /// codes, then the page's body arm picks the handler table — a Command
    /// runs its command handler (none: NO_HANDLER_REGISTERED), a Fact its
    /// fact handler (none: NO_FACT_HANDLER, so a fact never reaches a
    /// command handler or the reverse) — before rebuild (a corrupt persisted
    /// event fails the command). The emitted book passes through as the
    /// handler returned it: the coordinator stamps the cover, sequences and
    /// meta. A fact handler's record becomes the book: the fact, then its
    /// flags, payloads only.
    pub fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        let Some(command_book) = req.command.as_ref() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_COMMAND_BOOK,
                messages::NO_COMMAND_PAGES,
                [],
            ));
        };
        let Some(page) = command_book.page.as_ref() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_COMMAND_PAGE,
                messages::NO_COMMAND_PAGES,
                [],
            ));
        };
        let events = match &page.body {
            Some(pb::command_page::Body::Command(command)) => {
                let any = non_empty_payload(command.payload.as_ref())?;
                let Some(thunk) = self.handlers.get(crate::type_name_from_url(&any.type_url))
                else {
                    return Err(CodedError::unknown_type(
                        codes::NO_HANDLER_REGISTERED,
                        &any.type_url,
                        [],
                    ));
                };
                let (mut state, info) = self.rebuilder.rebuild(req.events.as_ref())?;
                let cctx = CommandContext {
                    next_sequence: crate::next_sequence(req.events.as_ref()),
                    had_prior_events: info.had_prior_events,
                    cover: command_book.cover.clone(),
                };
                thunk(any, &mut state, cctx)
                    .map_err(map_handler_error)?
                    .unwrap_or_default()
            }
            Some(pb::command_page::Body::Fact(fact)) => {
                let any = non_empty_payload(fact.payload.as_ref())?;
                let Some(thunk) = self.facts.get(crate::type_name_from_url(&any.type_url)) else {
                    return Err(CodedError::unknown_type(
                        codes::NO_FACT_HANDLER,
                        &any.type_url,
                        [],
                    ));
                };
                let (state, _) = self.rebuilder.rebuild(req.events.as_ref())?;
                let record = thunk(any, &state).map_err(map_handler_error)?;
                pb::EventBook {
                    pages: std::iter::once(record.fact)
                        .chain(record.flags)
                        .map(|payload| pb::EventPage {
                            payload: Some(payload),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                }
            }
            None => {
                return Err(CodedError::invalid_argument(
                    codes::MISSING_COMMAND_PAYLOAD,
                    messages::NO_COMMAND_PAGES,
                    [],
                ));
            }
        };
        Ok(pb::CommandHandlerResponse {
            events: Some(events),
            ..Default::default()
        })
    }
}

/// A body arm's payload, refused MISSING_COMMAND_PAYLOAD when absent or
/// untyped.
fn non_empty_payload(payload: Option<&Any>) -> Result<&Any, CodedError> {
    match payload {
        Some(any) if !any.type_url.is_empty() => Ok(any),
        _ => Err(CodedError::invalid_argument(
            codes::MISSING_COMMAND_PAYLOAD,
            messages::NO_COMMAND_PAGES,
            [],
        )),
    }
}

#[cfg(test)]
#[path = "aggregate.test.rs"]
mod aggregate_tests;
