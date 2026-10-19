# Design: rules version, conflict resolution, command upcasting and router-owned state

**Status:**
- **Implemented:** conflict resolution, command upcasting, the rules version
  on every response, and claims for upcaster source types at registration.
- **Implemented:** router-owned state across the C ABI for aggregates,
  process managers and projectors, and the retirement of `host_ctx`.
- **In progress:** the registration hello (`component_descriptor()`), per the owner rulings
  recorded under "Registration and projectors" below.
- **Open:** how the rules version is sourced (the owner intends the binary's
  full build version, emitted by versionator).

Snapshot selection is the coordinator's job (decision 14.1). The router treats
a delivered snapshot as the initial state and never judges it. ABI_VERSION stays
at 4.

## Spec

Source: angzarr-project `spec/20261019-draft` (619933c).

- **Protos:**
  - `command_handler.proto`: `CommandHandlerService.Handle`,
    `CommandHandlerResponse`.
  - `types.proto`: `Handling`, `ConflictResolution`, `ContextualCommand`,
    `ComponentDescriptor.rules_version`.
  - `options.proto`: the `conflict_units` region.
  - `upcaster.proto`.
- **Decisions:** 12.6, 12.7, 12.11, 14.1, 14.2.
- **Backlog:** RV-2, VC-1, NC-1, CR-1.
- **Behaviour:** the handler-contract tier,
  `features/handler-contract/handling.feature` (C-0533..C-0550). This repo's
  conformance suite binds those scenarios by id.

## What `Handle` does, in order

1. **Envelope guards** run first.
2. **Upcast.** The payload of either arm (command or fact) runs through the
   claimant aggregate's upcaster chains, by type URL (VC-1).
3. **Dispatch.** The body arm and the upcast type URL pick the handler. If no
   handler claims the type, the router answers INVALID_ARGUMENT. The message
   names the type URL as it was sent. If a chain ran, the extras carry
   `upcast_type_url`.
4. **Conflicts** (Command arm only):
   - **STRICT:** no extra step.
   - **AGGREGATE_HANDLES:** the handler receives the window and nonce. If it
     does not call `resolve_conflicts` with exactly that nonce, the command
     fails `CONFLICT_UNRESOLVED` (ABORTED). An empty `resolved_nonce` counts
     as unresolved.
   - **COMMUTATIVE:** the router runs these steps:
     1. Fold to the expected sequence and capture.
     2. Fold the window and capture the head.
     3. Decide on the head, then restore the head.
     4. Fold the command's events and capture.
     5. If `changed(expected, head)` and `changed(head, after)` overlap, fail
        with `CONFLICT_OVERLAP` (ABORTED).

     If no `changed` is registered, the command fails
     `CONFLICT_RESOLUTION_UNSUPPORTED` (INVALID_ARGUMENT). If an emitted event
     fails to fold, the error is INTERNAL.
5. **Rules version.** The router always sets the binary's declared version on
   every aggregate and PM response, overwriting whatever a handler set.
   Co-hosted PMs share one binary, so they share one version, and the router
   reports it even when no PM consumed the trigger.

### Claims

At registration, `CommandHandlers` claims each aggregate's command types and the
source types of its upcaster chains (`PayloadUpcaster::source_types`).

- Two claims of one (domain, type) are `DUPLICATE_COMMAND_HANDLER`. That
  includes an old type claimed by two aggregates, and an old type that
  collides with a current one.
- A fact goes to the first aggregate of its domain that records its type or
  accepts it upcast.

## Rust API

Native aggregates keep typed state.

| Symbol | Purpose |
|---|---|
| `with_rules_version` (aggregate and PM); `ProcessManagerDispatch::rules_version`; `ProcessManagerHandler::rules_version` | The binary's declared version, reported on every response. |
| `with_commutative(changed)` (`S: Clone`); `with_commutative_via(capture, restore, changed)` | Turns on COMMUTATIVE resolution. `_via` stays public by owner ruling. |
| `with_command_upcaster(impl PayloadUpcaster)` | Adds an upcaster chain. Its `source_types` are claimed at registration. |
| `CommandContext.{expected_sequence, conflict_resolution, conflict}`, `aggregate::Conflict` | The resolved handling, plus the window and nonce under AGGREGATE_HANDLES. |
| `CommandHandler::upcast_types` | The old types an aggregate claims. Default: empty. |
| `Rebuilder::with_fold(fold)` | A batched fold, `fold(&mut S, &[EventPage], cover)`, called once over every uncovered page. It replaces per-type appliers. The C ABI uses it over `prost_types::Any` state. |
| `conflict::overlap` | The overlap rule. |
| Error codes | `CONFLICT_UNRESOLVED` and `CONFLICT_OVERLAP` (both ABORTED); `CONFLICT_RESOLUTION_UNSUPPORTED` (INVALID_ARGUMENT). |

## C ABI: router-owned state (ABI 4, language-neutral)

The router holds the state of each C-ABI aggregate and process manager as a
serialized `google.protobuf.Any`. The spec requires state to be a protobuf
message, and its type URL is registered. Every host callback is stateless.

- **Initial state:** the delivered snapshot's `state`. Pages at or below the
  snapshot's sequence count as covered. With no snapshot, the initial state is
  `Any { type_url: "/" + state_type_url, value: [] }`, which is the message's
  default.
- **fold:**
  - Input: `FoldCall { Any state = 1; repeated EventPage pages = 2; Cover cover = 3; }`.
  - Output: the new state, as a serialized `Any`.
  - The router sends one call per rebuild, carrying every uncovered page.
  - `STATUS_OK_EMPTY` means the state is unchanged.
  - A failure is a handler bug: `UNHANDLED_HANDLER_ERROR` (INTERNAL), never
    `PERSISTED_EVENT_CORRUPT`, because the same apply logic accepted those
    events (owner ruling, 2026-10-19). The ErrorInfo metadata names the
    failing event's `sequence` and `type_url` and the `state_type_url`. To
    find the event, the router refolds the failed batch one page at a time
    from the same starting state (the fold is stateless, so this is safe);
    the first page that fails alone is named, or the batch's last page when
    every page folds alone.
  - An undecodable answer is INTERNAL too.
- **decide** (one callback per command type): the payload is the command; the
  aux is `CommandContextAux`, which now carries `state = 8`. The callback
  returns `CommandOutput { events, resolved_nonce }` and no state.
- **fact** (one callback per fact type): the payload is the fact; the aux is
  `CommandContextAux`, including its `state`. The callback returns
  `FactRecord`.
- **changed:** takes `ChangedCall { before, after }` (two state Anys) and
  returns `ChangedUnits { units }`. COMMUTATIVE copies state bytes, so it needs
  neither a state packer nor a snapshot loader.
- **PM event** (one callback per (input domain, type)): the aux is
  `PmEventAux { trigger_cover = 2; state = 3 }`. The callback returns
  `ProcessManagerHandleResponse`.

### `abi.proto` changes

| Message | Added | Removed and reserved |
|---|---|---|
| `AggregateDescriptor` | `state_type_url = 11`, `fold_callback_id = 12` | `appliers = 4`, `snapshot_callback_id = 6` (both were published in ABI 3) |
| `ProcessManagerDescriptor` | `state_type_url = 10`, `fold_callback_id = 11` | `appliers = 3`, `snapshot_callback_id = 4` |
| `CommandContextAux` | `state = 8` | none |
| `PmEventAux` | `state = 3` | none |
| `FoldCall` | new message | none |

Some removals carry no reservation because those fields were never published:

- `pack_state_callback_id = 12` was added and dropped within this ABI 4 round,
  so field 12 now holds `fold_callback_id`.
- Field 11 was never used before `state_type_url`.
- `INCOMPLETE_COMMUTATIVE` is gone.

`ProjectorEventAux` is still the aux for projector folds.

**What a binding does:** it registers a fold, its deciders, its fact handlers
and `changed` against the state message. It keeps no session state for
aggregates or PMs. Projectors are unchanged; see Open.

## Rejected alternatives

- **A re-entrant `resolve_conflicts` entry point.** The acknowledgement travels
  as data in `CommandOutput` instead.
- **A COMMUTATIVE resolver in each binding.** The overlap rule is implemented
  once, in Rust.
- **Per-event fold callbacks across the C ABI.** The router makes one batched
  call per rebuild.
- **decide returning a state.** decide stays pure; the router only folds
  events.
- **Trial upcasting at dispatch.** Replaced by claims at registration.

## Rulings applied

- **Stale results:** two codes, both ABORTED.
- **COMMUTATIVE without `changed`:** `CONFLICT_RESOLUTION_UNSUPPORTED`.
- **Upcasting:** applies to both arms. A refusal names the sent type, and the
  type a chain produced goes in the extras.
- **Old types:** claimed at registration.
- **Rules version:** always the declared one, never the handler's. PMs on one
  host share it.
- **Accepted:**
  - envelope guards run before upcasting;
  - an empty `resolved_nonce` means unresolved;
  - a COMMUTATIVE fold failure is INTERNAL;
  - any host fold failure is INTERNAL, naming the failing event (above).
- **State across the C ABI:** router-owned.
- **Snapshot selection:** the coordinator's (decision 14.1).

## Registration and projectors

These follow the owner rulings on the two open items, against spec b412af8 (now pinned at 619933c, which changes nothing the
router does).

### Registration hello: `component_descriptor()`

The router builds the spec's `ComponentDescriptor` once, in Rust, for every
language. Hosts only send it.

- **Fields.**
  - `name`.
  - `kind`: AGGREGATE, SAGA, PROCESS_MANAGER or PROJECTOR.
  - `inputs`: the domains whose events the component consumes, one
    `Target` per domain, deduplicated and sorted:
    - a saga: its input domain;
    - a process manager: its source domains;
    - a projector: its declared domains, or `*` when it declares none;
    - an aggregate: none, because it consumes commands and no events.
  - `rules_version`.
  - `state`: set for aggregates and process managers only.
    - `type_url` is `"/" + S::full_name()` for a native state type `S:
      prost::Name`, so nobody declares the string by hand. Across the C ABI it
      is the registered `state_type_url`.
    - `options.conflict_resolutions` is the full resolved list in enum-number
      order. The base is STRICT and AGGREGATE_HANDLES; COMMUTATIVE is added
      first when a `changed()` is registered (`with_commutative`).
- **Refusal.** A descriptor with an empty `rules_version` is refused
  `INVALID_ARGUMENT` (`MISSING_RULES_VERSION`), mirroring the coordinator.
  Every kind needs one, so sagas and projectors gain `with_rules_version`
  too.
- **Rust API.**
  - `AggregateDispatch::component_descriptor()` and
    `ProcessManagerDispatch::component_descriptor()`, both with
    `S: prost::Name`.
  - `..._with_state(state_type_url)` for state held as `Any`, which the C ABI
    uses.
  - `SagaDispatch::component_descriptor()` and
    `ProjectorDispatch::component_descriptor()`.
- **C ABI.**
  - New entry point: `angzarr_router_component_descriptors(r, out)`. It fills
    `out` with a serialized `ComponentDescriptors { repeated
    io.angzarr.v1.ComponentDescriptor components = 1; }` in registration
    order, or with a status: the first refusal.
  - The saga and projector descriptors gain `rules_version`.
- **Binding.** C-0535 binds against the native order aggregate.

### Router-owned projectors

Projectors now follow the aggregate and process-manager model. Each delivery
starts from the projection message's default, `Any { type_url: "/" +
projection_type_url, value: [] }`.

- For a declared domain, one stateless host fold, `FoldCall { state:
  projection, pages, cover }`, returns the new projection `Any`. An undeclared
  domain folds nothing.
- The router answers the `Projection` itself, so the host needs no finish
  step:
  - `cover`: the book's cover;
  - `projector`: the projector's name;
  - `projection`: the folded `Any`;
  - `next_sequence`: the last projected page's sequence + 1, or the book's
    `next_sequence` when it has no pages (types.proto, `Projection`).
- **Core.**
  - `ProjectorDispatch::with_fold(fold)` is the batched fold, used in place of
    the per-type folds.
  - `ProjectorDispatch::finish` stays for native projectors.

### `host_ctx` retired

With aggregates, process managers and projectors all router-owned, bindings
keep no state of any kind.

- **Removed:**
  - the per-dispatch `host_ctx` parameter, from every
    `angzarr_router_dispatch*` entry point and from the host callback
    `AngzarrCb`;
  - the thread-local session pointer behind it.
- **Unchanged:** a callback is still selected by its `callback_id`.

### `abi.proto` changes in this step

| Message | Added | Removed |
|---|---|---|
| `ProjectorDescriptor` | `projection_type_url = 6`, `fold_callback_id = 7`, `rules_version = 8` | `events = 3` and `finish_callback_id = 4`, both reserved because ABI 3 published them |
| `SagaDescriptor` | `rules_version = 6` | none |
| `ComponentDescriptors` | new message | none |
| `ProjectorEventAux` | none | the whole message, which nothing uses any more |

## Open

Nothing is open from the earlier rounds. C-0535 and the C-ABI projectors are
settled above.
