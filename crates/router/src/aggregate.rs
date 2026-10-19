//! AggregateDispatch — the command → events table.
//!
//! `CommandHandlerService.Handle` is the only input path: the body arm of the
//! ContextualCommand's page picks the pipeline (Command → a command handler,
//! Fact → a fact handler) and the payload's type URL picks the handler. The
//! envelope, the arm and the type validate BEFORE state is rebuilt (an
//! unknown command must report NO_HANDLER_REGISTERED, not whatever the
//! rebuild would surface). There is no compensation input.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use prost_types::Any;

use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
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
    /// The resolved `Handling.expected_sequence`: the head the command was
    /// issued against (stamped by the coordinator when the caller sent none).
    pub expected_sequence: u64,
    /// The resolved `Handling.conflict_resolution`.
    pub conflict_resolution: pb::ConflictResolution,
    /// The conflict to resolve: `Some` only under AGGREGATE_HANDLES. Under
    /// STRICT there is no window, and under COMMUTATIVE the router resolves
    /// the window itself, so the handler never sees the nonce.
    pub conflict: Option<Conflict>,
}

/// A stale command's conflict under AGGREGATE_HANDLES (types.proto,
/// ContextualCommand.conflict_nonce; backlog NC-1): the window of events the
/// command did not see, and the nonce the handler must hand back through
/// [`Conflict::resolve_conflicts`] before returning. A handler that does not
/// (or hands back another value) fails the command stale: its output is
/// discarded and the command answered CONFLICT_UNRESOLVED (ABORTED).
#[derive(Debug, Clone, Default)]
pub struct Conflict {
    window: Vec<pb::EventPage>,
    nonce: Vec<u8>,
    resolved: Arc<Mutex<Option<Vec<u8>>>>,
}

impl PartialEq for Conflict {
    fn eq(&self, other: &Self) -> bool {
        self.window == other.window && self.nonce == other.nonce
    }
}

impl Conflict {
    /// A conflict over `window` with the delivered `nonce`.
    pub fn new(window: Vec<pb::EventPage>, nonce: Vec<u8>) -> Self {
        Conflict {
            window,
            nonce,
            resolved: Arc::default(),
        }
    }

    /// The events the command did not see: the pages at sequences
    /// `expected_sequence..head`, in order.
    pub fn window(&self) -> &[pb::EventPage] {
        &self.window
    }

    /// The nonce delivered with the command.
    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    /// The handler's declaration that it examined the window (the spec's
    /// `resolve_conflicts(window, nonce)`). Only `nonce` is checked: it must
    /// be exactly the delivered value.
    pub fn resolve_conflicts(&self, window: &[pb::EventPage], nonce: &[u8]) {
        let _ = window;
        *self.resolved.lock().unwrap_or_else(|e| e.into_inner()) = Some(nonce.to_vec());
    }

    /// True when the handler resolved with exactly the delivered nonce.
    pub fn is_resolved(&self) -> bool {
        self.resolved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
            == Some(self.nonce.as_slice())
    }
}

/// The COMMUTATIVE resolver over one state instance (options.proto, the
/// conflict_units region, "The resolver"): captures the state at the
/// expected sequence and at the head, lets decide run on the head, restores
/// the head, folds the command's events, captures that, and reports the two
/// unit sets `changed(expected, head)` and `changed(head, after)`.
trait Resolver<S>: Send + Sync {
    fn resolve(
        &self,
        state: &mut S,
        fold_window: &mut dyn FnMut(&mut S) -> Result<(), CodedError>,
        decide: &mut dyn FnMut(&mut S) -> Result<pb::EventBook, CodedError>,
        fold_events: &mut dyn FnMut(&mut S, &pb::EventBook) -> Result<(), CodedError>,
    ) -> Result<Resolved, CodedError>;
}

/// What the resolver produced: decide's events and the two unit sets.
struct Resolved {
    events: pb::EventBook,
    window: Vec<String>,
    mine: Vec<String>,
}

type CaptureFn<S, I> = Box<dyn Fn(&S) -> Result<I, HandlerError> + Send + Sync>;
type RestoreFn<S, I> = Box<dyn Fn(&mut S, &I) -> Result<(), HandlerError> + Send + Sync>;
type ChangedFn<I> = Box<dyn Fn(&I, &I) -> Result<Vec<String>, HandlerError> + Send + Sync>;

struct CommutativeOps<S, I> {
    capture: CaptureFn<S, I>,
    restore: RestoreFn<S, I>,
    changed: ChangedFn<I>,
}

impl<S, I> Resolver<S> for CommutativeOps<S, I> {
    fn resolve(
        &self,
        state: &mut S,
        fold_window: &mut dyn FnMut(&mut S) -> Result<(), CodedError>,
        decide: &mut dyn FnMut(&mut S) -> Result<pb::EventBook, CodedError>,
        fold_events: &mut dyn FnMut(&mut S, &pb::EventBook) -> Result<(), CodedError>,
    ) -> Result<Resolved, CodedError> {
        let expected = (self.capture)(state).map_err(map_handler_error)?;
        fold_window(state)?;
        let head = (self.capture)(state).map_err(map_handler_error)?;
        let events = decide(state)?;
        (self.restore)(state, &head).map_err(map_handler_error)?;
        fold_events(state, &events)?;
        let after = (self.capture)(state).map_err(map_handler_error)?;
        Ok(Resolved {
            window: (self.changed)(&expected, &head).map_err(map_handler_error)?,
            mine: (self.changed)(&head, &after).map_err(map_handler_error)?,
            events,
        })
    }
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
    commutative: Option<Box<dyn Resolver<S>>>,
    upcasters: Vec<Box<dyn crate::PayloadUpcaster>>,
    rules_version: String,
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
            commutative: None,
            upcasters: Vec::new(),
            rules_version: String::new(),
        }
    }

    /// The rules version reported on every response
    /// (CommandHandlerResponse.rules_version; decision 12.6, backlog RV-2): the
    /// business binary's full version string, baked in at build. Empty until
    /// declared.
    pub fn with_rules_version(mut self, rules_version: impl Into<String>) -> Self {
        self.rules_version = rules_version.into();
        self
    }

    /// Supports COMMUTATIVE conflict resolution with the state's generated
    /// `changed(before, after)` (backlog CR-1): the router folds the state to
    /// the expected sequence and to the head, runs decide on the head, and
    /// keeps the command's events iff `changed(expected, head)` and
    /// `changed(head, head + events)` do not overlap ([`crate::conflict`]);
    /// otherwise the command fails CONFLICT_OVERLAP (ABORTED).
    pub fn with_commutative(
        self,
        changed: impl Fn(&S, &S) -> Vec<String> + Send + Sync + 'static,
    ) -> Self
    where
        S: Clone + Send + Sync + 'static,
    {
        self.with_commutative_via(
            |state: &S| Ok(state.clone()),
            |state: &mut S, image: &S| {
                *state = image.clone();
                Ok(())
            },
            move |before: &S, after: &S| Ok(changed(before, after)),
        )
    }

    /// [`Self::with_commutative`] over state images of type `I` rather than
    /// clones of the state, for state the router cannot clone (a host's,
    /// across the C ABI): `capture` takes an image of the current state,
    /// `restore` puts an image back, and `changed` compares two images.
    pub fn with_commutative_via<I: 'static>(
        mut self,
        capture: impl Fn(&S) -> Result<I, HandlerError> + Send + Sync + 'static,
        restore: impl Fn(&mut S, &I) -> Result<(), HandlerError> + Send + Sync + 'static,
        changed: impl Fn(&I, &I) -> Result<Vec<String>, HandlerError> + Send + Sync + 'static,
    ) -> Self
    where
        S: 'static,
    {
        self.commutative = Some(Box::new(CommutativeOps {
            capture: Box::new(capture),
            restore: Box::new(restore),
            changed: Box::new(changed),
        }));
        self
    }

    /// Adds an upcaster chain run on every incoming command and fact payload
    /// before dispatch, by type URL (upcaster.proto; decision 12.11). Chains
    /// run in registration order, each one's output feeding the next.
    pub fn with_command_upcaster(
        mut self,
        upcaster: impl crate::PayloadUpcaster + 'static,
    ) -> Self {
        self.upcasters.push(Box::new(upcaster));
        self
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
    /// codes; the payload upcast through the upcaster chains; the page's
    /// body arm picks the handler table — a Command runs its command handler
    /// (none: NO_HANDLER_REGISTERED), a Fact its fact handler (none:
    /// NO_FACT_HANDLER, so a fact never reaches a command handler or the
    /// reverse) — before rebuild (a corrupt persisted event fails the
    /// command). A refusal names the type URL the client sent, and with an
    /// upcaster chain its extras carry the last type URL the chain produced.
    /// A command is then decided under its resolved conflict resolution
    /// ([`Self::decide`]). The emitted book passes through as the handler
    /// returned it: the coordinator stamps the cover, sequences and meta. A
    /// fact handler's record becomes the book: the fact, then its flags,
    /// payloads only.
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
                let sent = non_empty_payload(command.payload.as_ref())?;
                let any = self.upcast(sent)?;
                let Some(thunk) = self.handlers.get(crate::type_name_from_url(&any.type_url))
                else {
                    return Err(self.unknown(codes::NO_HANDLER_REGISTERED, sent, &any));
                };
                self.decide(req, command_book, &any, thunk)?
            }
            Some(pb::command_page::Body::Fact(fact)) => {
                let sent = non_empty_payload(fact.payload.as_ref())?;
                let any = self.upcast(sent)?;
                let Some(thunk) = self.facts.get(crate::type_name_from_url(&any.type_url)) else {
                    return Err(self.unknown(codes::NO_FACT_HANDLER, sent, &any));
                };
                let (state, _) = self.rebuilder.rebuild(req.events.as_ref())?;
                let record = thunk(&any, &state).map_err(map_handler_error)?;
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
            rules_version: self.rules_version.clone(),
        })
    }

    /// The payload run through every upcaster chain, in registration order.
    pub fn upcast_payload(&self, sent: &Any) -> Result<Any, CodedError> {
        self.upcast(sent)
    }

    fn upcast(&self, sent: &Any) -> Result<Any, CodedError> {
        let mut current = sent.clone();
        for upcaster in &self.upcasters {
            current = upcaster.upcast_payload(&current)?;
        }
        Ok(current)
    }

    /// The refusal of a type no handler of the arm's kind claims: the message
    /// names the type URL the client sent; with an upcaster chain, the extras
    /// also carry the last type URL the chain produced.
    fn unknown(&self, code: &str, sent: &Any, upcast: &Any) -> CodedError {
        let produced = (!self.upcasters.is_empty())
            .then(|| (extras::UPCAST_TYPE_URL.to_string(), upcast.type_url.clone()));
        CodedError::unknown_type(code, &sent.type_url, produced)
    }

    /// Runs a command's handler under its resolved conflict resolution
    /// (types.proto, Handling and ConflictResolution):
    /// - STRICT (or unspecified): decide on the rebuilt head;
    /// - AGGREGATE_HANDLES: decide on the head with the window and nonce in
    ///   [`CommandContext::conflict`]; unless the handler resolved with the
    ///   delivered nonce, its output is discarded: CONFLICT_UNRESOLVED;
    /// - COMMUTATIVE: the registered resolver ([`Self::with_commutative`])
    ///   decides on the head and keeps the events iff the window's units and
    ///   the command's do not overlap (else CONFLICT_OVERLAP); with none
    ///   registered, CONFLICT_RESOLUTION_UNSUPPORTED, before rebuild.
    fn decide(
        &self,
        req: &pb::ContextualCommand,
        command_book: &pb::CommandBook,
        any: &Any,
        thunk: &CommandFn<S>,
    ) -> Result<pb::EventBook, CodedError> {
        let events = req.events.as_ref();
        let next_sequence = crate::next_sequence(events);
        let handling = command_book.handling.unwrap_or_default();
        let expected_sequence = handling.expected_sequence.unwrap_or(next_sequence);
        let conflict_resolution = pb::ConflictResolution::try_from(handling.conflict_resolution)
            .unwrap_or(pb::ConflictResolution::Unspecified);
        let context = |had_prior_events: bool, conflict: Option<Conflict>| CommandContext {
            next_sequence,
            had_prior_events,
            cover: command_book.cover.clone(),
            expected_sequence,
            conflict_resolution,
            conflict,
        };
        let run = |state: &mut S, cctx: CommandContext| -> Result<pb::EventBook, CodedError> {
            Ok(thunk(any, state, cctx)
                .map_err(map_handler_error)?
                .unwrap_or_default())
        };
        match conflict_resolution {
            pb::ConflictResolution::AggregateHandles => {
                let (mut state, info) = self.rebuilder.rebuild(events)?;
                let conflict = Conflict::new(
                    window_pages(events, expected_sequence),
                    req.conflict_nonce.to_vec(),
                );
                let out = run(
                    &mut state,
                    context(info.had_prior_events, Some(conflict.clone())),
                )?;
                if !conflict.is_resolved() {
                    return Err(CodedError::stale(
                        codes::CONFLICT_UNRESOLVED,
                        messages::CONFLICT_UNRESOLVED,
                        [],
                    ));
                }
                Ok(out)
            }
            pb::ConflictResolution::Commutative => {
                let Some(resolver) = self.commutative.as_ref() else {
                    return Err(CodedError::invalid_argument(
                        codes::CONFLICT_RESOLUTION_UNSUPPORTED,
                        messages::CONFLICT_RESOLUTION_UNSUPPORTED,
                        [],
                    ));
                };
                let had_prior_events =
                    events.is_some_and(|b| !b.pages.is_empty() || b.snapshot.is_some());
                let cover = events.and_then(|b| b.cover.as_ref());
                let (prefix, window) = split_at_expected(events, expected_sequence);
                let (mut state, _) = self.rebuilder.rebuild(Some(&prefix))?;
                let resolved = resolver.resolve(
                    &mut state,
                    &mut |state| {
                        for page in &window {
                            self.rebuilder.apply_page(state, page, cover)?;
                        }
                        Ok(())
                    },
                    &mut |state| run(state, context(had_prior_events, None)),
                    &mut |state, book| {
                        for page in &book.pages {
                            self.rebuilder.apply_page(state, page, cover).map_err(|e| {
                                CodedError::unhandled(&format!(
                                    "an emitted event does not fold into state: {}",
                                    e.message
                                ))
                            })?;
                        }
                        Ok(())
                    },
                )?;
                if crate::conflict::overlap(&resolved.window, &resolved.mine) {
                    return Err(CodedError::stale(
                        codes::CONFLICT_OVERLAP,
                        messages::CONFLICT_OVERLAP,
                        [
                            (extras::WINDOW_UNITS.to_string(), resolved.window.join(",")),
                            (extras::COMMAND_UNITS.to_string(), resolved.mine.join(",")),
                        ],
                    ));
                }
                Ok(resolved.events)
            }
            _ => {
                let (mut state, info) = self.rebuilder.rebuild(events)?;
                run(&mut state, context(info.had_prior_events, None))
            }
        }
    }
}

/// The conflict window: the pages at or after `expected` (types.proto,
/// ContextualCommand: `expected_sequence .. next_sequence-1`).
fn window_pages(events: Option<&pb::EventBook>, expected: u64) -> Vec<pb::EventPage> {
    events
        .map(|b| {
            b.pages
                .iter()
                .filter(|p| p.sequence >= expected)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The prior-events book up to `expected` (its snapshot and earlier pages)
/// and the window after it.
fn split_at_expected(
    events: Option<&pb::EventBook>,
    expected: u64,
) -> (pb::EventBook, Vec<pb::EventPage>) {
    let Some(book) = events else {
        return (pb::EventBook::default(), Vec::new());
    };
    let (window, prefix): (Vec<_>, Vec<_>) = book
        .pages
        .iter()
        .cloned()
        .partition(|p| p.sequence >= expected);
    (
        pb::EventBook {
            pages: prefix,
            ..book.clone()
        },
        window,
    )
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
