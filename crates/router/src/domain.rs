//! DomainDispatch — a domain's command → events table: an aggregate's,
//! or a process manager's, since a process manager is a domain (decision
//! 14.31), served by the same table with its start rule
//! ([`DomainDispatch::process_manager`]).
//!
//! Two calls per input, both pure: [`DomainDispatch::admit`] applies the
//! rules an input meets before its handler runs (deduplication by its id,
//! the edition fork rule, a process manager's start rule), from what the
//! runtime read of the addressed stream; then
//! [`DomainDispatch::dispatch`] runs the handler. The runtime (annals'
//! kernel, or core's coordinator) owns the store reads and writes between
//! and after them.
//!
//! `CommandHandlerService.Handle` is the only input path: the body arm of the
//! ContextualCommand's page picks the pipeline (Command → a command handler,
//! Fact → a fact handler) and the payload's type URL picks the handler. The
//! envelope, the arm and the type validate BEFORE state is rebuilt (an
//! unknown command must report NO_HANDLER_REGISTERED, not whatever the
//! rebuild would surface). There is no compensation input.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use prost::Message;
use prost_types::Any;

use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
use crate::pb;
use crate::rebuild::{PageOrigin, Rebuilder};

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

/// What the runtime knows when an input reaches a domain, before its
/// handler runs: the input and what it read of the addressed stream.
#[derive(Debug, Clone, Copy)]
pub struct Arrival<'a> {
    /// The input as delivered: its cover names the addressed stream
    /// (domain, edition, root); its identity is stamped (command_id; and
    /// causation_id for a saga's command or a refusal fact).
    pub command: &'a pb::CommandBook,
    /// The events the addressed stream already holds under this input's id
    /// ([`crate::input_id`]; the store's `find_command`), in sequence
    /// order. Empty when none.
    pub recorded: &'a [pb::EventPage],
    /// The addressed stream, as read in the input's edition.
    pub stream: StreamStatus,
}

/// The addressed stream as the runtime read it in the input's edition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamStatus {
    /// The stream holds any event, as read in the input's edition: for a
    /// process manager, its instance exists (process_manager.proto,
    /// Starting an instance).
    pub exists: bool,
    /// In a named edition: the stream has a fork there (decision 14.32).
    /// Ignored for main, which is never forked.
    pub forked: bool,
    /// The stream's next sequence as read in the input's edition (main's
    /// head, for a stream not forked there): the fork sequence of an
    /// arrival fork.
    pub next_sequence: u64,
}

/// What [`DomainDispatch::admit`] decides for an input. A refusal is the
/// `Err` beside it.
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    /// Run the handler ([`DomainDispatch::dispatch`]).
    Dispatch,
    /// A saga's command reaching a stream with no fork in its edition: the
    /// runtime appends this framework event at the stream's head, then
    /// runs the handler (types.proto, EditionForked, Propagation).
    /// `propagated_from` is left for the runtime, which knows the fork
    /// event of the stream the saga reacted to.
    Fork(pb::EditionForked),
    /// Already applied: the runtime answers these events, DUPLICATE, and the
    /// handler does not run (types.proto, the command_identity region).
    Duplicate(Vec<pb::EventPage>),
    /// A process manager's fact for a correlation with no instance, of a
    /// type it does not declare as starting: skipped silently, answered
    /// APPLIED with no events (process_manager.proto).
    Skip,
}

/// Handles a passed-through refusal (the standard `CommandRefused`) of one
/// refused command type against the rebuilt state: the source domain's
/// `on_refused_<Command>`.
pub type RefusedFn<S> =
    Box<dyn Fn(&pb::CommandRefused, &S) -> Result<FactRecord, HandlerError> + Send + Sync>;

/// One fact handler, bound to its input: records it against the state.
type RecordFn<'a, S> = Box<dyn Fn(&S) -> Result<FactRecord, CodedError> + 'a>;

/// A process manager's start rule: whether an input payload (as sent) is of
/// a type that may start an instance, judged after upcasting. `None` for an
/// aggregate, which has none.
pub type StartRule<'a> = Option<&'a dyn Fn(&Any) -> Result<bool, CodedError>>;

/// The dispatch table for one domain: an aggregate or a process manager.
pub struct DomainDispatch<S> {
    name: String,
    domain: String,
    kind: pb::ComponentKind,
    rebuilder: Rebuilder<S>,
    handlers: HashMap<String, CommandFn<S>>,
    facts: HashMap<String, FactFn<S>>,
    refused: HashMap<String, RefusedFn<S>>,
    starting: BTreeSet<String>,
    commutative: Option<Box<dyn Resolver<S>>>,
    upcasters: Vec<Box<dyn crate::PayloadUpcaster>>,
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
            starting: BTreeSet::new(),
            commutative: None,
            upcasters: Vec::new(),
            rules_version: String::new(),
        }
    }

    /// An empty process-manager table over a Rebuilder: a domain (decision
    /// 14.31) whose inputs are commands and facts, whose only output is its
    /// own events, whose root is its correlation id, and which declares its
    /// starting inputs ([`Self::starts`]). Registered with kind
    /// PROCESS_MANAGER and no inputs (it subscribes to nothing).
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

    /// Declares a registered command or fact type (fully qualified) as one
    /// that may start a process-manager instance (options.proto,
    /// CommandOptions.starts; advertised as
    /// ComponentDescriptor.starting_types). An aggregate declaring one is
    /// refused at registration (STARTING_TYPES_ON_NON_PM).
    pub fn starts(mut self, full_name: &str) -> Self {
        self.starting.insert(full_name.to_string());
        self
    }

    /// Registers the handler of a refusal fact passed through by one of the
    /// domain's sagas: the standard `CommandRefused` whose refused command
    /// is of the fully-qualified type `refused_command` (another domain's
    /// command), the generated `on_refused_<Command>` dispatch (types.proto,
    /// the refusal_facts region; decision 14.31). An ordinary, infallible
    /// fact handler: it receives the decoded `CommandRefused` and records
    /// its events. A `CommandRefused` for a command type with none is
    /// NO_FACT_HANDLER.
    pub fn on_refused(
        mut self,
        refused_command: &str,
        thunk: impl Fn(&pb::CommandRefused, &S) -> Result<FactRecord, HandlerError>
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

    /// The rules an input meets before its handler runs, decided from the
    /// [`Arrival`] alone (no I/O), in order:
    /// 1. validation: a refusal fact (a Fact with causation_id) is accepted
    ///    only by the exact stream its causation_id names
    ///    (REFUSAL_FACT_WRONG_STREAM); an input to a process manager whose
    ///    set correlation_id differs from its cover's root is malformed
    ///    (PM_CORRELATION_NOT_ROOT);
    /// 2. deduplication: events already recorded under the input's id are
    ///    [`Admission::Duplicate`];
    /// 3. editions: in a named edition, a stream with no fork there takes a
    ///    saga's command by forking ([`Admission::Fork`]) and refuses any
    ///    other input FAILED_PRECONDITION, EDITION_NOT_FORKED;
    /// 4. a process manager's start rule, by the input's type after
    ///    upcasting: with no instance, a starting type is dispatched; any
    ///    other command is refused FAILED_PRECONDITION, PM_NO_INSTANCE, and
    ///    any other fact is [`Admission::Skip`].
    pub fn admit(&self, arrival: &Arrival<'_>) -> Result<Admission, CodedError> {
        if self.kind != pb::ComponentKind::ProcessManager {
            return admit_rules(arrival, None);
        }
        let starts = |sent: &Any| -> Result<bool, CodedError> {
            let any = self.upcast(sent)?;
            Ok(self
                .starting
                .contains(crate::type_name_from_url(&any.type_url)))
        };
        admit_rules(arrival, Some(&starts))
    }

    /// The rules version reported on every response
    /// (CommandHandlerResponse.rules_version; decision 12.6, backlog RV-2): the
    /// business binary's full version string, baked in at build. Empty until
    /// declared.
    pub fn with_rules_version(mut self, rules_version: impl Into<String>) -> Self {
        self.rules_version = rules_version.into();
        self
    }

    /// The registration hello (types.proto, ComponentDescriptor): kind
    /// AGGREGATE, no inputs (an aggregate consumes commands, not events), and
    /// the state type its rebuilder declares. Refused MISSING_RULES_VERSION
    /// without a rules version, MISSING_STATE_TYPE without a state type.
    pub fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        if self.kind != pb::ComponentKind::ProcessManager && !self.starting.is_empty() {
            return Err(CodedError::invalid_argument(
                codes::STARTING_TYPES_ON_NON_PM,
                messages::STARTING_TYPES_ON_NON_PM,
                [(extras::COMPONENT.to_string(), self.name.clone())],
            ));
        }
        crate::descriptor::component_descriptor(
            &self.name,
            self.kind,
            [],
            &self.rules_version,
            Some(crate::descriptor::state_descriptor(
                &self.name,
                self.rebuilder.state_type_url(),
                self.commutative.is_some(),
            )?),
            self.starting.iter().cloned().collect(),
        )
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

    /// Checks the aggregate's own claims: an old type its upcaster chains
    /// accept that is also one of its current command types is
    /// DUPLICATE_COMMAND_HANDLER (the old type would never reach its chain).
    pub fn check_claims(&self) -> Result<(), CodedError> {
        match self
            .upcast_types()
            .into_iter()
            .find(|old| self.handlers.contains_key(old))
        {
            Some(old) => Err(CodedError::invalid_argument(
                codes::DUPLICATE_COMMAND_HANDLER,
                messages::DUPLICATE_COMMAND_HANDLER,
                [
                    (extras::DOMAIN.to_string(), self.domain.clone()),
                    (extras::COMMAND_TYPE.to_string(), old),
                ],
            )),
            None => Ok(()),
        }
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
    /// command). A refusal names the type URL the client sent; its extras
    /// carry the type the chains ended at as `upcast_type_url`.
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
        let cover = req.events.as_ref().and_then(|b| b.cover.as_ref());
        let (mut events, state) = match &page.body {
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
                let fact_type = crate::type_name_from_url(&any.type_url);
                let record_with: RecordFn<'_, S> = match self.facts.get(fact_type) {
                    Some(thunk) => {
                        let any = any.clone();
                        Box::new(move |state| thunk(&any, state).map_err(map_handler_error))
                    }
                    None if fact_type == <pb::CommandRefused as prost::Name>::full_name() => {
                        let refused =
                            pb::CommandRefused::decode(any.value.as_slice()).map_err(|_| {
                                CodedError::invalid_argument(
                                    codes::ANY_DECODE_FAILED,
                                    "failed to decode CommandRefused",
                                    [(extras::TYPE_URL.to_string(), any.type_url.clone())],
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
                            return Err(CodedError::unknown_type(
                                codes::NO_FACT_HANDLER,
                                &sent.type_url,
                                &any.type_url,
                                [(extras::COMMAND_TYPE.to_string(), refused_type)],
                            ));
                        };
                        Box::new(move |state| thunk(&refused, state).map_err(map_handler_error))
                    }
                    None => return Err(self.unknown(codes::NO_FACT_HANDLER, sent, &any)),
                };
                let (mut state, _) = self.rebuilder.rebuild(req.events.as_ref())?;
                let record = record_with(&state)?;
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
                (recorded, state)
            }
            None => {
                return Err(CodedError::invalid_argument(
                    codes::MISSING_COMMAND_PAYLOAD,
                    messages::NO_COMMAND_PAGES,
                    [],
                ));
            }
        };
        // The post-command snapshot, on every answer (owner ruling): the
        // coordinator stamps its sequence and rules version.
        if let Some(image) = self.rebuilder.state_image(&state) {
            events.snapshot = Some(pb::Snapshot {
                state: Some(image),
                ..Default::default()
            });
        }
        Ok(pb::CommandHandlerResponse {
            events: Some(events),
            rules_version: self.rules_version.clone(),
        })
    }

    /// The fully-qualified types its upcaster chains accept, claimed for it
    /// at registration like its command types.
    pub fn upcast_types(&self) -> Vec<String> {
        let mut types: Vec<String> = self
            .upcasters
            .iter()
            .flat_map(|u| u.source_types())
            .collect();
        types.sort();
        types.dedup();
        types
    }

    fn upcast(&self, sent: &Any) -> Result<Any, CodedError> {
        let mut current = sent.clone();
        for upcaster in &self.upcasters {
            current = upcaster.upcast_payload(&current)?;
        }
        Ok(current)
    }

    /// The refusal of a type no handler of the arm's kind claims: the
    /// message names the type URL the client sent, the extras the type the
    /// upcaster chains ended at (`upcast_type_url`).
    fn unknown(&self, code: &str, sent: &Any, upcast: &Any) -> CodedError {
        CodedError::unknown_type(code, &sent.type_url, &upcast.type_url, [])
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
    ///
    /// Answers decide's events and the state after them: the head with the
    /// events folded in (origin Emitted).
    fn decide(
        &self,
        req: &pb::ContextualCommand,
        command_book: &pb::CommandBook,
        any: &Any,
        thunk: &CommandFn<S>,
    ) -> Result<(pb::EventBook, S), CodedError> {
        let events = req.events.as_ref();
        let next_sequence = crate::next_sequence(events);
        let cover = events.and_then(|b| b.cover.as_ref());
        let fold_emitted = |state: &mut S, out: &pb::EventBook| {
            self.rebuilder
                .apply_emitted(state, &out.pages, cover, next_sequence)
        };
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
                fold_emitted(&mut state, &out)?;
                Ok((out, state))
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
                let (prefix, window) = split_at_expected(events, expected_sequence);
                let (mut state, _) = self.rebuilder.rebuild(Some(&prefix))?;
                let resolved = resolver.resolve(
                    &mut state,
                    &mut |state| {
                        self.rebuilder
                            .apply_pages(state, &window, cover, PageOrigin::Persisted)
                    },
                    &mut |state| run(state, context(had_prior_events, None)),
                    &mut |state, book| fold_emitted(state, book),
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
                // The resolver leaves the state at the head plus the
                // command's events.
                Ok((resolved.events, state))
            }
            _ => {
                let (mut state, info) = self.rebuilder.rebuild(events)?;
                let out = run(&mut state, context(info.had_prior_events, None))?;
                fold_emitted(&mut state, &out)?;
                Ok((out, state))
            }
        }
    }
}

/// The rules every domain's input meets before its handler runs
/// ([`DomainDispatch::admit`]), with a process manager's `start_rule`
/// (`None` for an aggregate, or a hand-written domain with no start rule).
pub fn admit_rules(
    arrival: &Arrival<'_>,
    start_rule: StartRule<'_>,
) -> Result<Admission, CodedError> {
    let book = arrival.command;
    let Some(page) = book.page.as_ref() else {
        return Err(CodedError::invalid_argument(
            codes::MISSING_COMMAND_PAGE,
            messages::NO_COMMAND_PAGES,
            [],
        ));
    };
    let (is_fact, sent) = match &page.body {
        Some(pb::command_page::Body::Command(c)) => (false, c.payload.as_ref()),
        Some(pb::command_page::Body::Fact(f)) => (true, f.payload.as_ref()),
        None => (false, None),
    };
    let sent = non_empty_payload(sent)?;
    let cover = book.cover.clone().unwrap_or_default();
    let edition = cover.edition.as_deref().filter(|e| !e.is_empty());

    // 1. Validation.
    if let (true, Some(cause)) = (is_fact, page.causation_id.as_ref()) {
        let named = cause.stream.clone().unwrap_or_default();
        let same = named.domain == cover.domain
            && named.root == cover.root
            && named.edition.as_deref().filter(|e| !e.is_empty()) == edition;
        if !same {
            return Err(CodedError::invalid_argument(
                codes::REFUSAL_FACT_WRONG_STREAM,
                messages::REFUSAL_FACT_WRONG_STREAM,
                [(extras::DOMAIN.to_string(), cover.domain.clone())],
            ));
        }
    }
    if start_rule.is_some() {
        if let Some(correlation) = page.correlation_id.as_ref() {
            if Some(correlation) != cover.root.as_ref() {
                return Err(CodedError::invalid_argument(
                    codes::PM_CORRELATION_NOT_ROOT,
                    messages::PM_CORRELATION_NOT_ROOT,
                    [(extras::DOMAIN.to_string(), cover.domain.clone())],
                ));
            }
        }
    }

    // 2. Deduplication.
    if !arrival.recorded.is_empty() {
        return Ok(Admission::Duplicate(arrival.recorded.to_vec()));
    }

    // 3. Editions: a saga's input (one with a causation_id) forks the stream
    // on arrival; any other is refused until the stream is forked.
    let fork = match edition {
        Some(edition) if !arrival.stream.forked => {
            if page.causation_id.is_none() {
                return Err(CodedError::rejection_precondition_failed(
                    codes::EDITION_NOT_FORKED,
                    messages::EDITION_NOT_FORKED,
                    [(extras::DOMAIN.to_string(), cover.domain.clone())],
                ));
            }
            Some(arrival_fork(
                edition,
                arrival.stream.next_sequence,
                page.command_id.clone(),
            ))
        }
        _ => None,
    };

    // 4. A process manager's start rule.
    if let Some(starts) = start_rule {
        if !arrival.stream.exists && !starts(sent)? {
            if is_fact {
                return Ok(Admission::Skip);
            }
            return Err(CodedError::rejection_precondition_failed(
                codes::PM_NO_INSTANCE,
                messages::PM_NO_INSTANCE,
                [(extras::TYPE_URL.to_string(), sent.type_url.clone())],
            ));
        }
    }

    Ok(fork.map_or(Admission::Dispatch, Admission::Fork))
}

/// The fork a saga's input makes on arriving at a stream with no fork in
/// its edition (types.proto, EditionForked, Propagation): at the stream's
/// head, `forked_by` the arriving input's id. `propagated_from` is the
/// runtime's to fill. The one place the router builds the fork event, so
/// the spec's coming rename of it is a change here.
fn arrival_fork(edition: &str, head: u64, forked_by: Option<pb::Uuid>) -> pb::EditionForked {
    pb::EditionForked {
        edition: edition.to_string(),
        parent_sequence: head,
        commit_position: 0,
        propagated_from: None,
        forked_by,
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
#[path = "domain.test.rs"]
mod domain_tests;

#[cfg(test)]
#[path = "admit.test.rs"]
mod admit_tests;
