# angzarr-router architecture

angzarr-router is the Rust client library's router, and the reference
router: the one part of a client library that calls business code
(decision 14.42). It routes an input to the business code registered for
its type, folds the state it is given, runs decide or ingest, and answers
what business code did, as the framework's messages. Python has a native
router of its own; angzarr-project's handler-contract tier
(`features/handler-contract/`, with its fixtures in `fixture/`) is the
complete contract of both, run in CI against every router.

The router has no transport and no store. The host process serves the
coordinator's gRPC services (`CommandHandlerService`, `PolicyService`,
`ProjectorService`, `ComponentService`) and hands their messages to the
router; the coordinator (core's, or the angzarr-embedded kernel) owns
every read and write around each call.

## What the router does, and what it leaves to the coordinator

| The router (this crate) | The coordinator |
|---|---|
| routes by type URL; refuses a type nothing handles | upcasts every payload and event first, through the component's upcaster sidecar (14.39), and relays the type as sent beside an unknown type (`upcast_type_url`) |
| folds the starting state it is delivered and every page, in order | loads the snapshot, drops the pages it covers, decides when to write one (14.40) |
| answers the state after every input, with the build's rules version | stamps the snapshot's sequence and rules version, and the events' |
| — | admits an input: deduplication, the refusal fact's stream check, a process manager's start rule by `starting_types` |
| answers `Describe` with the descriptor it was given, unchanged | validates the descriptor angzarr-cli generated |
| answers a status and a reason key with its args | owns its own keys (`CoordinatorReason`), and classifies every status (14.37's four classes) |
| answers a policy's commands as business code returned them, grouped by triggering page | checks each command's destination, edition, root and correlation, stamps the edition and identity |
| runs a translating callback for a refusal, or answers none | builds the pass-through `CommandRefused`, addresses and identifies every refusal fact |
| — | builds every delivery well formed (no envelope guards in the router) |
| reports a stale command's conflicts | judges the report: `COORDINATOR_REASON_CONFLICT_UNRESOLVED`, `COORDINATOR_REASON_CONFLICT_OVERLAP` |

## The API

`crates/router` is a public contract, documented on each item. Business
payloads are opaque `Any`s; the router calls thunks keyed by
fully-qualified type name.

- `Rebuilder<S>`: a state factory, per-type appliers (`apply`,
  `apply_with_context`) or one batched fold (`with_fold`), the state type
  (`with_state_type`), and the state's one codec (`with_state_codec`:
  decode the starting state, encode the state answered).
  `Rebuilder::of_message` takes all of them from a prost message.
- `DomainDispatch<S>`: an aggregate (`new`) or a process manager
  (`process_manager`; a PM is a domain, decision 14.31): command handlers
  (`on_command`), fact handlers (`on_fact`, `on_fact_with_context`),
  pass-through refusal handlers (`on_refused`), COMMUTATIVE support
  (`with_commutative`, `with_commutative_via`) and the rules version
  (`with_rules_version`).
- `PolicyDispatch`: one input domain and one output domain, event handlers
  (`on_event`, `on_event_with_context`) and translating callbacks for
  refused commands (`on_refused`).
- `ProjectorDispatch<P>`: the domains it consumes (`for_domains`, `*` for
  every one), event folds (`on_event`) or one batched fold (`with_fold`),
  and a finish step (`finish`).
- `RouterBuilder`/`Router`: one component per router (owner ruling: one
  coordinator, one component, one business process), and the descriptor
  it answers `Describe` with (`RouterBuilder::descriptor`,
  `Router::component_descriptor`). The traits `CommandHandler`,
  `PolicyComponent` and `ProjectorHandler` let a runtime hold a component
  of its own (one in another process, say). They are kernel-facing and
  async (owner ruling 2026-10-30): their dispatch returns a future, `Send`
  on every target but wasm32 (`send::MaybeSend`), and so do the `Router`'s
  `dispatch_*`. The router's own components answer at once, a ready future
  around the business code's synchronous functions; business code is never
  async. Each trait has an object-safe twin (`DynCommandHandler`,
  `DynPolicyComponent`, `DynProjectorHandler`, boxed futures) that every
  implementor is, and whose `dyn` is the trait again: components are held
  and registered boxed as `Box<dyn Dyn…>`.
- `CodedError`, `HandlerError` and the reason keys (`error::RouterReason`,
  re-exported from angzarr-proto), and the wire types re-exported as `pb`.

## Semantics

| Semantic | Contract |
|---|---|
| Folding | A delivered starting state (`EventBook.snapshot.state`) is decoded by the state's codec (`with_state_codec`; `of_message` decodes the state message); without one it is ignored. Then every page the book holds folds onto it, in order, whatever its sequence (14.40); pageless or unknown-type entries are skipped, never terminal. A starting state the codec cannot decode is `ROUTER_REASON_PERSISTED_EVENT_CORRUPT` (DATA_LOSS); a codec that panics is `ROUTER_REASON_FOLD_FAILED` naming the snapshot's `sequence` and `type_url`. A page that fails to fold is a handler bug (INTERNAL, `ROUTER_REASON_FOLD_FAILED`, or `ROUTER_REASON_FOLD_FAILED_EMITTED` for a fold of the command's own events), whose metadata names the page's `sequence` and `type_url` (or the batch's `first_sequence` and `last_sequence`), the `state_type_url`, and the fold's own failure, when it is a key and args, as `cause` and `cause.<arg>` |
| Domain dispatch | `CommandHandlerService.Handle` is the only input: the page's body arm picks the table (a Command its command handler, a Fact its fact handler), the payload's type the handler, before any fold. A type nothing handles is `ROUTER_REASON_UNKNOWN_COMMAND_TYPE` or `ROUTER_REASON_NO_FACT_HANDLER` (INTERNAL), naming `type_url`, the type it was given; a delivery with no book, page, arm or payload is a command of no type. Handlers receive a `CommandContext` (next sequence, had-prior-events, the cover, the resolved Handling, the input's identity and `recorded_at`); the emitted book passes through as the handler returned it |
| Facts | A fact handler sees the folded state and returns the fact to record and the events that flag it; they come back as the answer's events, payloads only. A `CommandRefused` runs the domain's `on_refused` handler for the refused command's type (none: `ROUTER_REASON_NO_REFUSAL_HANDLER`; one that does not decode: `ROUTER_REASON_ANY_DECODE_FAILED`) |
| State on every answer | Every domain answer carries `events.snapshot.state`, the state after its events (folded at the sequences they will get), encoded by the codec; sequence and rules version stay unset for the coordinator. A rejection has no body |
| Rules version | Every `CommandHandlerResponse` carries the domain's declared rules version (`with_rules_version`) |
| Conflicts | Handlers see the resolved `Handling`. AGGREGATE_HANDLES hands the window and nonce in `CommandContext.conflict`, and the answer reports the nonce business code passed to `resolve_conflicts` (empty if none) in `CommandHandlerResponse.conflict`. COMMUTATIVE runs the registered resolver over the generated `changed()`: decide on the head, and report `changed(expected, head)` and `changed(head, head + events)`. Either way the events are answered; the coordinator judges. A COMMUTATIVE delivery with no resolver is `ROUTER_REASON_CONFLICT_RESOLUTION_UNSUPPORTED` |
| Policy | EVERY page of the source book dispatches; undeclared types are skipped (C-0051). The answer holds one `PolicyReaction` per handled page, in page order, an empty one included, each carrying the commands as business code returned them (C-0690). A handler sees the triggering page's cover, sequence and metadata. OnRefused (`dispatch_refusal`) answers the translating callback's fact payload alone, as a fact page with no cover, handling or identity (C-0691), or no fact: no translation (C-0692) |
| Projector | Folds every page into one projection instance, each fold seeing the cover and the page's sequence; undeclared domains fold nothing (C-0032), `*` consumes every domain; unmatched types are skipped with a WARN; without a finish step the `Projection` carries the cover, the projector's name and its next sequence. A failing fold is named as a state fold's is, with the projection type as `state_type_url` |
| Describe | `Router::component_descriptor` answers the `ComponentDescriptor` the router was given (angzarr-cli generates it at build time), unchanged, and none when given none |
| One component | A router hosts one component: none is `ROUTER_REASON_NO_HANDLERS_REGISTERED`, a second of any kind `ROUTER_REASON_DUPLICATE_COMPONENT` (both INTERNAL; the latter naming the hosted `component` and `component_kind`); dispatching a kind it does not host is `ROUTER_REASON_NO_COMPONENT_OF_KIND` (UNIMPLEMENTED) |
| Type URLs | Emitted type URLs use the bare `/` prefix; incoming ones match by the fully-qualified name after the last `/`, whatever the prefix |
| Errors | Decision 14.37: every error is a key and an argument map, never English. The status message IS the key; the `google.rpc.ErrorInfo` (domain `angzarr.io`) carries the same key and the args as metadata; a wrapped cause flattens to `cause` / `cause.<arg>`; `Display` is the key then ` name=value` args. The router's keys are reasons.proto's `RouterReason` (decision 14.44), answered as the full value name (`as_str_name()`); a test holds the router to answering every value but `ROUTER_REASON_UNSPECIFIED`, and no retired one. A handler refuses with `rejection(BusinessStatus, key, args)` or a typed `refused(object)` (`ROUTER_REASON_COMMAND_REFUSED`, `refusal_type`), returns at once with `immediate(key, args)` (ABORTED), or fails as a `bug(key, args)` or a `retryable_bug(status, key, args)` (`ROUTER_REASON_UNHANDLED_HANDLER_ERROR`, its key the `cause`); an unclassified error is `ROUTER_REASON_UNHANDLED_HANDLER_ERROR` with no cause, and a panic is caught (`cause` `ROUTER_REASON_HANDLER_PANICKED`), their text left to the log. The router never classifies: what a status means is the coordinator's |

Not provided here: gRPC serving, health and readiness, transport
configuration, CloudEvents, and every duty the table above gives the
coordinator.

## Testing

- `just test`: the unit bank (`crates/router/src/*.test.rs`) and the
  handler-contract tier, run whole from the submodule by cucumber-rs
  (`crates/conformance`, one harness per tier feature).
- `just mutation-test`: cargo-mutants over the router's modules and the
  architecture linter, every missed mutant a failure; weekly in CI.
  `just mutation-push`, by hand, is the same over only what a push adds.
  No git hook runs mutation.
- `just push-check` (pre-push): every test, in each pushed commit's tree
  checked out alone. Red tests pushed to a branch pass with a warning;
  pushed to the default branch they are refused.
- `just archlint`: the layering as checked rules (`archlint.toml`: the
  router below the harness, its components independent of each other and
  of the `Router` that composes them, the harness on the public surface),
  enforced by `tools/archlint` over every module's references and every
  manifest dependency. Runs in pre-commit and CI.
