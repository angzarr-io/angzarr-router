//! DomainDispatch — a domain's command → events table: an aggregate's,
//! or a process manager's, since a process manager is a domain (decision
//! 14.31), served by the same table ([`DomainDispatch::process_manager`]).
//! There is no trigger-fed process manager:
//!
//! ```compile_fail
//! use angzarr_router::process_manager::ProcessManagerDispatch;
//! ```
//!
//! One call per input, pure: [`DomainDispatch::dispatch`] runs the handler.
//! The runtime (the angzarr-embedded kernel, or core's coordinator) owns the
//! store reads and writes around it, and admits the input first.
//!
//! The router never sees a meta-event and has no edition-lifecycle outcome
//! (owner ruling, 2026-10-28): creating, forking, refusing an unforked or
//! deleted edition's arrival and deleting are the runtime's, decided before
//! it calls the router.
//!
//! `CommandHandlerService.Handle` is the only input path: the body arm of the
//! ContextualCommand's page picks the pipeline (Command → a command handler,
//! Fact → a fact handler) and the payload's type URL picks the handler. The
//! envelope, the arm and the type validate BEFORE state is rebuilt (an
//! unknown command must report ROUTER_REASON_UNKNOWN_COMMAND_TYPE, not whatever the
//! rebuild would surface). There is no compensation input.
//!
//! The router admits nothing (decision 14.42): deduplication, the refusal
//! fact's stream check and a process manager's start rule are the
//! coordinator's, done before it calls the router. No admit rules,
//!
//! ```compile_fail
//! use angzarr_router::domain::admit_rules;
//! ```
//!
//! no domain that admits,
//!
//! ```compile_fail
//! use angzarr_router::domain::{Admission, Arrival};
//! fn admits(domain: &dyn angzarr_router::router::CommandHandler, arrival: &Arrival<'_>) -> Admission {
//!     domain.admit(arrival).unwrap()
//! }
//! ```
//!
//! and no router that admits:
//!
//! ```compile_fail
//! use angzarr_router::domain::{Admission, Arrival};
//! fn admits(router: &angzarr_router::router::Router, arrival: &Arrival<'_>) -> Admission {
//!     router.admit(arrival).unwrap()
//! }
//! ```

use std::collections::HashMap;

use prost::Message;
use prost_types::Any;

use crate::error::{call_handler, extras, CodedError, HandlerError, RouterReason};
use crate::pb;
use crate::rebuild::{PageOrigin, Rebuilder};

pub use crate::context::{command_context, CommandContext, Conflict};

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
        let expected = call_handler(|| (self.capture)(state))?;
        fold_window(state)?;
        let head = call_handler(|| (self.capture)(state))?;
        let events = decide(state)?;
        call_handler(|| (self.restore)(state, &head))?;
        fold_events(state, &events)?;
        let after = call_handler(|| (self.capture)(state))?;
        Ok(Resolved {
            window: call_handler(|| (self.changed)(&expected, &head))?,
            mine: call_handler(|| (self.changed)(&head, &after))?,
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

/// Records one fact (a command page's Fact arm) against the rebuilt state,
/// reading the fact's [`CommandContext`] (its identity and origin).
/// Generated thunks unmarshal to the typed fact and pack the typed result.
pub type FactFn<S> =
    Box<dyn Fn(&Any, &S, CommandContext) -> Result<FactRecord, HandlerError> + Send + Sync>;

/// Handles a passed-through refusal (the standard `CommandRefused`) of one
/// refused command type against the rebuilt state, with the refusal fact's
/// [`CommandContext`] (its identity is the refused command's): the source
/// domain's `on_refused_<Command>`.
pub type RefusedFn<S> = Box<
    dyn Fn(&pb::CommandRefused, &S, CommandContext) -> Result<FactRecord, HandlerError>
        + Send
        + Sync,
>;

/// One fact handler, bound to its input: records it against the state.
type RecordFn<'a, S> = Box<dyn Fn(&S, CommandContext) -> Result<FactRecord, CodedError> + 'a>;

/// The dispatch table for one domain: an aggregate or a process manager.
pub struct DomainDispatch<S> {
    name: String,
    domain: String,
    kind: pb::ComponentKind,
    rebuilder: Rebuilder<S>,
    handlers: HashMap<String, CommandFn<S>>,
    facts: HashMap<String, FactFn<S>>,
    refused: HashMap<String, RefusedFn<S>>,
    commutative: Option<Box<dyn Resolver<S>>>,
    rules_version: String,
}

impl<S> DomainDispatch<S> {
    /// An empty aggregate table over a Rebuilder.
    pub fn new(
        name: impl Into<String>,
        domain: impl Into<String>,
        rebuilder: Rebuilder<S>,
    ) -> Self {
        DomainDispatch {
            name: name.into(),
            domain: domain.into(),
            kind: pb::ComponentKind::Aggregate,
            rebuilder,
            handlers: HashMap::new(),
            facts: HashMap::new(),
            refused: HashMap::new(),
            commutative: None,
            rules_version: String::new(),
        }
    }

    /// An empty process-manager table over a Rebuilder: a domain (decision
    /// 14.31) whose inputs are commands and facts, whose only output is its
    /// own events, and whose stream root is the one its input's cover names
    /// (decision 14.36). Its kind is PROCESS_MANAGER; its starting types are
    /// its generated descriptor's, which the coordinator's start rule reads
    /// (decision 14.42).
    pub fn process_manager(
        name: impl Into<String>,
        domain: impl Into<String>,
        rebuilder: Rebuilder<S>,
    ) -> Self {
        DomainDispatch {
            kind: pb::ComponentKind::ProcessManager,
            ..Self::new(name, domain, rebuilder)
        }
    }

    /// Registers the handler of a refusal fact passed through by one of the
    /// domain's policies: the standard `CommandRefused` whose refused command
    /// is of the fully-qualified type `refused_command` (another domain's
    /// command), the generated `on_refused_<Command>` dispatch (types.proto,
    /// the refusal_facts region; decision 14.31). An ordinary fact handler:
    /// it receives the decoded `CommandRefused` and the refusal fact's
    /// [`CommandContext`] (its command id and correlation the refused
    /// command's, its cover the stream of the policy's trigger), and records
    /// its events, or fails as any fact handler may. A `CommandRefused` for
    /// a command type with none is ROUTER_REASON_NO_FACT_HANDLER.
    pub fn on_refused(
        mut self,
        refused_command: &str,
        thunk: impl Fn(&pb::CommandRefused, &S, CommandContext) -> Result<FactRecord, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.refused
            .insert(refused_command.to_string(), Box::new(thunk));
        self
    }

    /// AGGREGATE or PROCESS_MANAGER.
    pub fn kind(&self) -> pb::ComponentKind {
        self.kind
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
    /// reports `changed(expected, head)` and `changed(head, head + events)`
    /// beside the command's events, for the coordinator to judge (decision
    /// 14.42).
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
        self,
        fq_fact_type: &str,
        thunk: impl Fn(&Any, &S) -> Result<FactRecord, HandlerError> + Send + Sync + 'static,
    ) -> Self {
        self.on_fact_with_context(fq_fact_type, move |fact, state, _cctx| thunk(fact, state))
    }

    /// Registers a fact handler, as [`Self::on_fact`], that also receives
    /// the fact's [`CommandContext`]: what a command handler receives
    /// (STRICT, no conflict), the fact's command id, correlation and origin
    /// (types.proto, CommandPage) included.
    pub fn on_fact_with_context(
        mut self,
        fq_fact_type: &str,
        thunk: impl Fn(&Any, &S, CommandContext) -> Result<FactRecord, HandlerError>
            + Send
            + Sync
            + 'static,
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

    /// `CommandHandlerService.Handle`: the page's body arm picks the handler
    /// table — a Command runs its command handler (none:
    /// ROUTER_REASON_UNKNOWN_COMMAND_TYPE), a Fact its fact handler (none: ROUTER_REASON_NO_FACT_HANDLER,
    /// so a fact never reaches a command handler or the reverse) — before
    /// rebuild (a corrupt persisted event fails the
    /// command). A refusal names the type URL it was given: the coordinator
    /// has upcast it (decision 14.39), and relays the type as sent beside it.
    /// A command is then decided under its resolved conflict resolution
    /// ([`Self::decide`]). The emitted book passes through as the handler
    /// returned it: the coordinator stamps the cover, sequences and meta. A
    /// fact handler's record becomes the book: the fact, then its flags,
    /// payloads only.
    ///
    /// It guards no envelope (decision 14.42): the coordinator delivers
    /// every input well formed. A delivery with no book, page, arm or
    /// payload is routed as a command of no type, ROUTER_REASON_UNKNOWN_COMMAND_TYPE.
    pub fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        let body = req
            .command
            .as_ref()
            .and_then(|b| b.page.as_ref())
            .and_then(|p| p.body.as_ref());
        let untyped = Any::default();
        let cover = req.events.as_ref().and_then(|b| b.cover.as_ref());
        let (mut events, state, conflict) = match body {
            Some(pb::command_page::Body::Fact(fact)) => {
                let any = fact.payload.as_ref().unwrap_or(&untyped);
                let fact_type = crate::type_name_from_url(&any.type_url);
                let record_with: RecordFn<'_, S> = match self.facts.get(fact_type) {
                    Some(thunk) => {
                        let any = any.clone();
                        Box::new(move |state, cctx| call_handler(|| thunk(&any, state, cctx)))
                    }
                    None if fact_type == <pb::CommandRefused as prost::Name>::full_name() => {
                        let refused =
                            pb::CommandRefused::decode(any.value.as_slice()).map_err(|e| {
                                CodedError::undecodable(
                                    &<pb::CommandRefused as prost::Name>::full_name(),
                                    &e,
                                )
                            })?;
                        let refused_type = refused
                            .command
                            .as_ref()
                            .and_then(|b| b.page.as_ref())
                            .and_then(crate::command_payload)
                            .map(|p| p.type_url.clone())
                            .unwrap_or_default();
                        let Some(thunk) =
                            self.refused.get(crate::type_name_from_url(&refused_type))
                        else {
                            return Err(CodedError::internal(
                                RouterReason::NoRefusalHandler,
                                [(extras::COMMAND_TYPE.to_string(), refused_type)],
                            ));
                        };
                        Box::new(move |state, cctx| call_handler(|| thunk(&refused, state, cctx)))
                    }
                    None => {
                        return Err(CodedError::unknown_type(
                            RouterReason::NoFactHandler,
                            &any.type_url,
                        ))
                    }
                };
                let (mut state, _) = self.rebuilder.rebuild(req.events.as_ref())?;
                let record = record_with(&state, command_context(req))?;
                let recorded = pb::EventBook {
                    pages: std::iter::once(record.fact)
                        .chain(record.flags)
                        .map(|payload| pb::EventPage {
                            payload: Some(payload),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                };
                self.rebuilder.apply_emitted(
                    &mut state,
                    &recorded.pages,
                    cover,
                    crate::next_sequence(req.events.as_ref()),
                )?;
                (recorded, state, None)
            }
            command => {
                let any = match command {
                    Some(pb::command_page::Body::Command(command)) => command.payload.as_ref(),
                    _ => None,
                }
                .unwrap_or(&untyped);
                let Some(thunk) = self.handlers.get(crate::type_name_from_url(&any.type_url))
                else {
                    return Err(CodedError::unknown_type(
                        RouterReason::UnknownCommandType,
                        &any.type_url,
                    ));
                };
                self.decide(req, any, thunk)?
            }
        };
        // The state after the input, on every answer (decision 14.12): the
        // coordinator decides whether to write it, and stamps its sequence
        // and rules version (decision 14.40).
        if let Some(after) = self.rebuilder.encode_state(&state) {
            events.snapshot = Some(pb::Snapshot {
                state: Some(after),
                ..Default::default()
            });
        }
        Ok(pb::CommandHandlerResponse {
            events: Some(events),
            rules_version: self.rules_version.clone(),
            conflict,
        })
    }

    /// Runs a command's handler under its resolved conflict resolution
    /// (types.proto, Handling and ConflictResolution):
    /// - STRICT (or unspecified): decide on the rebuilt head;
    /// - AGGREGATE_HANDLES: decide on the head with the window and nonce in
    ///   [`CommandContext::conflict`];
    /// - COMMUTATIVE: the registered resolver ([`Self::with_commutative`])
    ///   decides on the head and computes the window's units and the
    ///   command's; with none registered, ROUTER_REASON_CONFLICT_RESOLUTION_UNSUPPORTED,
    ///   before rebuild.
    ///
    /// It judges nothing (decision 14.42): the coordinator accepts the
    /// events only if the reported nonce is the one it delivered
    /// (COORDINATOR_REASON_CONFLICT_UNRESOLVED), or the two unit sets do not overlap
    /// (COORDINATOR_REASON_CONFLICT_OVERLAP).
    ///
    /// Answers decide's events, the state after them (the head with the
    /// events folded in, origin Emitted) and the conflict report (decision
    /// 14.42): under AGGREGATE_HANDLES the nonce the handler passed to
    /// `resolve_conflicts` (empty if it never called it), under COMMUTATIVE
    /// the window's units and the command's; none under STRICT.
    fn decide(
        &self,
        req: &pb::ContextualCommand,
        any: &Any,
        thunk: &CommandFn<S>,
    ) -> Result<(pb::EventBook, S, Option<pb::ConflictReport>), CodedError> {
        let events = req.events.as_ref();
        let next_sequence = crate::next_sequence(events);
        let cover = events.and_then(|b| b.cover.as_ref());
        let fold_emitted = |state: &mut S, out: &pb::EventBook| {
            self.rebuilder
                .apply_emitted(state, &out.pages, cover, next_sequence)
        };
        let base = command_context(req);
        let expected_sequence = base.expected_sequence;
        let conflict_resolution = base.conflict_resolution;
        let run = |state: &mut S, cctx: CommandContext| -> Result<pb::EventBook, CodedError> {
            Ok(call_handler(|| thunk(any, state, cctx))?.unwrap_or_default())
        };
        match conflict_resolution {
            pb::ConflictResolution::AggregateHandles => {
                let (mut state, _) = self.rebuilder.rebuild(events)?;
                let out = run(&mut state, base.clone())?;
                let report = pb::ConflictReport {
                    resolved_nonce: base
                        .conflict
                        .as_ref()
                        .map(Conflict::resolved_nonce)
                        .unwrap_or_default(),
                    ..Default::default()
                };
                fold_emitted(&mut state, &out)?;
                Ok((out, state, Some(report)))
            }
            pb::ConflictResolution::Commutative => {
                let Some(resolver) = self.commutative.as_ref() else {
                    return Err(CodedError::internal(
                        RouterReason::ConflictResolutionUnsupported,
                        [],
                    ));
                };
                let (prefix, window) = split_at_expected(events, expected_sequence);
                let (mut state, _) = self.rebuilder.rebuild(Some(&prefix))?;
                let resolved = resolver.resolve(
                    &mut state,
                    &mut |state| {
                        self.rebuilder
                            .apply_pages(state, &window, cover, PageOrigin::Persisted)
                    },
                    &mut |state| run(state, base.clone()),
                    &mut |state, book| fold_emitted(state, book),
                )?;
                // The resolver leaves the state at the head plus the
                // command's events.
                let report = pb::ConflictReport {
                    window_units: resolved.window,
                    command_units: resolved.mine,
                    ..Default::default()
                };
                Ok((resolved.events, state, Some(report)))
            }
            _ => {
                let (mut state, _) = self.rebuilder.rebuild(events)?;
                let out = run(&mut state, base.clone())?;
                fold_emitted(&mut state, &out)?;
                Ok((out, state, None))
            }
        }
    }
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

#[cfg(test)]
#[path = "domain.test.rs"]
mod domain_tests;
