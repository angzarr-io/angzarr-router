//! AggregateDispatch — the command → events table.
//!
//! Envelope and command type validate BEFORE state is rebuilt (an unknown
//! command must report NO_HANDLER_REGISTERED, not whatever the rebuild
//! would surface). There is no compensation input: a command whose type
//! has no registered handler reports NO_HANDLER_REGISTERED.

use std::collections::HashMap;

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

/// The dispatch table for one aggregate component.
pub struct AggregateDispatch<S> {
    name: String,
    domain: String,
    rebuilder: Rebuilder<S>,
    handlers: HashMap<String, CommandFn<S>>,
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
    /// command), thunk with rebuilt state and CommandContext. The emitted
    /// book passes through unstamped: the coordinator stamps sequences and
    /// metadata.
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
            },
        )
        .map_err(map_handler_error)?;

        Ok(pb::BusinessResponse {
            result: Some(pb::business_response::Result::Events(
                events.unwrap_or_default(),
            )),
        })
    }
}

#[cfg(test)]
#[path = "aggregate.test.rs"]
mod aggregate_tests;
