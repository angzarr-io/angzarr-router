# Design: rules version, conflict resolution, command upcasting and snapshot validity

**Status:**
- **Implemented, as signed off:** conflict resolution, command upcasting and
  the rules version reported on every response. Commits c33c930 and 6588184
  carry them, and `conformance/features/handling.feature` plus
  `crates/router/src/handling.test.rs` pin them.
- **Open:** how the rules version is sourced. The owner intends the business
  binary to bake its full version string in at build time (versionator emits
  it), and the router just reports whatever it is given.
- **Superseded:** snapshot validity. The coordinator decides which snapshot to
  offer, from a startup hello in which the business logic sends its version.
  The router validates no snapshot and never answers `snapshot_refused`.

ABI_VERSION stays at 4.

## Spec

These are from angzarr-project `spec/20261019-draft`:
- `command_handler.proto` (`CommandHandlerService.Handle`,
  `CommandHandlerResponse`);
- `types.proto` (`Handling`, `ConflictResolution`, `ContextualCommand`);
- `options.proto` (the `conflict_units` region);
- `upcaster.proto`.

The governing decisions are 12.6, 12.7 and 12.11, and the backlog items are
RV-2, HD-5, NC-1, CR-1 and VC-1. The project features have no scenario ids for
these yet.

## What `Handle` does, in order

1. **Upcast (VC-1).** The payload of either arm (Command or Fact) runs through
   the component's upcaster chains, by type URL.
2. **Dispatch.** The body arm and the upcast type URL pick the handler. If no
   handler claims that type, the command is refused `INVALID_ARGUMENT`:
   - the message names the type URL the client sent;
   - with an upcaster chain, the extras also carry the last type URL the chain
     produced, under `upcast_type_url`.
3. **Conflicts (HD-5, NC-1, CR-1)**, for the Command arm only (a fact is never
   conflict-resolved):
   - **STRICT** (or unspecified): decide runs on the head.
   - **AGGREGATE_HANDLES:** decide runs on the head with the window and nonce.
     If the handler does not call `resolve_conflicts` with the delivered nonce,
     its output is discarded with `CONFLICT_UNRESOLVED` (ABORTED).
   - **COMMUTATIVE:** the router:
     1. folds the state to the expected sequence and captures it;
     2. folds the window and captures the head;
     3. runs decide on the head;
     4. restores the head, which discards anything decide did to the state;
     5. folds the command's events and captures the result;
     6. keeps the events if `changed(expected, head)` and `changed(head,
        after)` do not overlap, and otherwise answers `CONFLICT_OVERLAP`
        (ABORTED).

     With no `changed` registered the answer is
     `CONFLICT_RESOLUTION_UNSUPPORTED` (INVALID_ARGUMENT), returned before
     rebuild.
4. **Rules version.** Every response reports the component's rules version
   (RV-2).

`CommandHandlers` (and so the Rust `Router` and the C ABI registry) routes an
old type by first trying the sent type. Failing that, it tries each aggregate
of the domain whose chain brings the type to one that aggregate claims. Failing
that, the domain's first aggregate gets it and produces the refusal.

## Rust API

| Symbol | Purpose |
|---|---|
| `AggregateDispatch::with_rules_version` and `ProcessManagerDispatch::with_rules_version` | The string reported on every response. A process manager with none declared keeps its handler's own value. |
| `AggregateDispatch::with_commutative(changed)` (needs `S: Clone`) | COMMUTATIVE through the generated `changed` (backlog CR-1). |
| `AggregateDispatch::with_commutative_via(capture, restore, changed)` | The same, over state images, for state the router can't clone (host state behind the C ABI). |
| `AggregateDispatch::with_command_upcaster(impl PayloadUpcaster)` | An upcaster chain for command and fact payloads. `UpcasterDispatch` implements `PayloadUpcaster`. |
| `CommandContext.expected_sequence`, `conflict_resolution`, `conflict` | The resolved `Handling`. `conflict` is `Some` only under AGGREGATE_HANDLES. |
| `aggregate::Conflict` | `window()`, `nonce()`, `resolve_conflicts(window, nonce)` (only the nonce is checked), `is_resolved()`. |
| `CommandHandler::upcast_payload` | Has a default. Routing consults it so an old type reaches the aggregate its chain serves. |
| `conflict::overlap` | The spec's overlap rule. |
| `codes::CONFLICT_UNRESOLVED`, `CONFLICT_OVERLAP` | Both ABORTED; `GrpcCode::Aborted` is new. |
| `codes::CONFLICT_RESOLUTION_UNSUPPORTED` | INVALID_ARGUMENT. |
| `codes::INCOMPLETE_COMMUTATIVE` | INVALID_ARGUMENT, at C ABI registration. |
| `extras::UPCAST_TYPE_URL`, `WINDOW_UNITS`, `COMMAND_UNITS` | Extras keys on the refusals above. |

```rust
AggregateDispatch::new("list", "list", rebuilder)
    .with_rules_version(env!("CARGO_PKG_VERSION"))   // sourcing: open
    .with_commutative(ListState::changed)             // generated (CR-1)
    .with_command_upcaster(command_upcasters)
    .on_command("hatpick.v1.Rename", |cmd, state, cctx| {
        if let Some(conflict) = &cctx.conflict {      // AGGREGATE_HANDLES only
            /* examine conflict.window() */
            conflict.resolve_conflicts(conflict.window(), conflict.nonce());
        }
        Ok(Some(events))
    })
```

## C ABI (ABI 4, language-neutral)

### `abi.proto` additions

- **`AggregateDescriptor`** gains:
  - `rules_version = 10`;
  - `pack_state_callback_id = 12`: the host packs its session state into an
    `Any`;
  - `changed_callback_id = 13`: receives a `ChangedCall { before, after }` and
    returns `ChangedUnits { units }`. Registration requires both the packer and
    `snapshot_callback_id`, which is how the router restores the head;
    otherwise it refuses with `INCOMPLETE_COMMUTATIVE`;
  - `command_upcasters = 14`: ordered rules, each returning a serialized `Any`.
    `STATUS_OK_EMPTY` means the payload is unchanged.
- **`ProcessManagerDescriptor`** gains `rules_version = 9`.
- **`CommandContextAux`** gains `expected_sequence = 4`,
  `conflict_resolution = 5`, and `conflict_nonce = 6` and `window = 7`, both
  set only under AGGREGATE_HANDLES.
- **A command callback's STATUS_OK output** is now `CommandOutput { events = 1,
  resolved_nonce = 2 }`. A host's `resolve_conflicts` sets `resolved_nonce`.

The C function signatures don't change. Field 11 of `AggregateDescriptor` is
unused: it was proposed for `state_type_url`, which the coordinator-side
snapshot design made unnecessary.

**A binding's role:**
- At registration it sends its generated rules version, `changed`, state
  packer and upcasters.
- For each command callback it builds the handler context from
  `CommandContextAux`. Calling `resolve_conflicts` on that context stores the
  nonce, and the binding returns it in `CommandOutput.resolved_nonce`.
- For COMMUTATIVE, the router drives the host's single session state through
  pack, restore (via the snapshot loader) and `changed`. The overlap rule runs
  once, in Rust.

## Rejected alternatives

- **A re-entrant `angzarr_router_resolve_conflicts` entry point:** it would
  re-enter the router mid-dispatch. The acknowledgement travels as data
  instead.
- **A COMMUTATIVE resolver in each binding:** that would implement the overlap
  rule once per language.
- **`Conflict::resolve_conflicts(nonce)` without the window:** it diverges from
  the spec's signature.
- **An upcaster as a separate router component:** a router hosts one kind, and
  the chain belongs to the aggregate's own `Handle`.

## Owner rulings applied

- **Stale outcomes:** two codes, both ABORTED. COMMUTATIVE with no `changed` is
  `CONFLICT_RESOLUTION_UNSUPPORTED` (INVALID_ARGUMENT).
- **Upcasting** runs on both arms. An unknown-after-upcast refusal names the
  sent type in the message and the chain's last type in the extras.
- **Process managers** use the existing merge rule: `rules_version` is the
  first non-empty value.
- **Snapshot validity** (superseded): the coordinator decides, via the startup
  hello. The router-side state-type check, rules-version comparison and
  `snapshot_refused` answers were never implemented, and their design tests
  were dropped.

## Decisions taken in implementation (not covered by the rulings)

- **Envelope guards come first.** `MISSING_COMMAND_BOOK`, `MISSING_COMMAND_PAGE`
  and `MISSING_COMMAND_PAYLOAD` still precede upcasting.
- **Failing to fold an emitted event** during the COMMUTATIVE `after` fold is
  `UNHANDLED_HANDLER_ERROR` (INTERNAL, a handler bug), not
  `PERSISTED_EVENT_CORRUPT`.
- **Routing of old types.** `CommandHandler::upcast_payload` was added so the
  shared-domain router can route an old type to the aggregate whose chain
  serves it.
- **A process manager with no declared rules version** keeps the value its
  handler returned. A declared value overrides it.
- **`resolved_nonce` empty** means "did not resolve". A host that leaves it
  empty fails stale even when the delivered nonce is empty, which the spec
  never sends under AGGREGATE_HANDLES.
- **`INCOMPLETE_COMMUTATIVE`** is a new registration-time code for
  `changed_callback_id` without both the packer and the snapshot loader.
