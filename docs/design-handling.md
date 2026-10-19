# Design: rules version, snapshot validity, conflict resolution and command upcasting

**Status:** proposal, pending owner sign-off. The tests that pin this API are
already in the tree and red. `crates/router/src/handling.test.rs` compiles
only with `--features pending-handling`, and
`conformance/features/handling.feature` is tagged `@wip` and has no runner yet.
Nothing below is implemented. ABI_VERSION stays at 4, because nothing has been
published since the bump.

## What the spec asks for

Everything here comes from angzarr-project `spec/20261019-draft`:
`command_handler.proto` (`CommandHandlerService.Handle`,
`CommandHandlerResponse`), `types.proto` (`Snapshot`, `Handling`,
`ConflictResolution`, `ContextualCommand`), the `conflict_units` region of
`options.proto`, and `upcaster.proto`. The governing decisions are 12.6, 12.7
and 12.11, and the backlog items are RV-2, RV-3, HD-5, NC-1, CR-1 and VC-1.
The project's feature files have no scenario ids for any of this yet; see
"Spec ambiguities" below.

Inside `Handle` the router (the "client library" in the spec's terms) works
through these steps in order:

1. **Snapshot validity (RV-3).** Before decoding a delivered snapshot, compare
   its state type URL and its `rules_version` with the component's own. On any
   mismatch, answer `snapshot_refused`. Nothing else is set, and no business
   code runs.
2. **Upcast (VC-1).** Run the payload through the component's upcaster chain,
   by type URL. A type URL that reaches no handler is refused
   `INVALID_ARGUMENT`, and the refusal names it. That refusal is already
   implemented (commit 965b61a).
3. **Dispatch.** The body arm and the type URL pick the handler. Already
   implemented.
4. **Conflicts (HD-5, NC-1, CR-1).**
   - **STRICT:** nothing to do.
   - **AGGREGATE_HANDLES:** decide receives the window and the nonce, and must
     call `resolve_conflicts` with exactly that nonce. Otherwise its output is
     discarded and the command fails `ABORTED`.
   - **COMMUTATIVE:** the library folds the state to `expected_sequence`, then
     to the head, and runs decide on the head. It compares
     `changed(expected, head)` against `changed(head, head + events)`. Disjoint
     unit sets mean the library acknowledges the nonce itself and keeps the
     events; overlapping sets mean `ABORTED`.

Every response also reports the component's `rules_version` (RV-2), for both
aggregates and process managers.

## Rust API (crates/router)

### New builder methods

| Symbol | Purpose |
|---|---|
| `AggregateDispatch::with_rules_version(impl Into<String>)` | The rules version reported on every response, and compared against a delivered snapshot's. |
| `AggregateDispatch::with_state_type(impl Into<String>)` | The state message's fully-qualified name, compared against the snapshot state's type URL. |
| `AggregateDispatch::with_commutative(changed)` where `S: Clone` | Supplies the generated `changed(before, after) -> Vec<String>` (backlog CR-1) and turns on the library resolver. |
| `AggregateDispatch::with_command_upcaster(UpcasterDispatch)` | Adds an event-style upcaster chain for command and fact payloads. Repeated calls run in registration order. |
| `ProcessManagerDispatch::with_rules_version` and `with_state_type` | The same checks for a process manager's `process_state` snapshot and its response. |

### New and changed types

- `CommandContext` gains three fields:
  - `expected_sequence: u64` and `conflict_resolution: pb::ConflictResolution`,
    both read from the resolved `Handling`;
  - `conflict: Option<aggregate::Conflict>`, which is `Some` only under
    AGGREGATE_HANDLES. It stays `None` under STRICT, and under COMMUTATIVE,
    where the library resolves and the handler never sees the nonce.
- `aggregate::Conflict` provides:
  - `window() -> &[pb::EventPage]`: the pages at sequences `E..head`;
  - `nonce() -> &[u8]`;
  - `resolve_conflicts(&self, window: &[pb::EventPage], nonce: &[u8])`, which
    records the acknowledgement. Only the nonce is checked. The window argument
    exists so the call has the spec's signature, `resolve_conflicts(window,
    nonce)`, in every language.
- `conflict::overlap(a: &[String], b: &[String]) -> bool` implements the
  spec's overlap rule: a wildcard against any non-empty set, equal units, or
  prefixes that end at a `.` or `[` boundary.

### New error codes

- `codes::CONFLICT_UNRESOLVED`: AGGREGATE_HANDLES output was discarded. Maps to
  ABORTED.
- `codes::CONFLICT_OVERLAP`: COMMUTATIVE found an overlap. Maps to ABORTED, and
  the extras carry both unit sets.
- `codes::CONFLICT_RESOLUTION_UNSUPPORTED`: a COMMUTATIVE delivery reached an
  aggregate that registered no `changed`. Maps to INVALID_ARGUMENT.
- `GrpcCode::Aborted = 10` is added, and `abi::grpc_from_wire` maps 10 to it.

### Response behaviour

- **Snapshot refused:** `Ok(CommandHandlerResponse { snapshot_refused: true,
  events: None, rules_version })`, and likewise
  `ProcessManagerHandleResponse.snapshot_refused`. A refusal is an answer, not
  an error.
- **Handler output kept:** `events` and `rules_version` are filled.

### A Rust handler author

```rust
AggregateDispatch::new("list", "list", rebuilder)
    .with_rules_version("v3")
    .with_state_type("hatpick.v1.ListState")
    .with_commutative(ListState::changed)        // generated (CR-1)
    .on_command("hatpick.v1.Rename", |cmd, state, cctx| {
        if let Some(conflict) = &cctx.conflict {   // AGGREGATE_HANDLES only
            /* examine conflict.window() */
            conflict.resolve_conflicts(conflict.window(), conflict.nonce());
        }
        Ok(Some(events))
    })
```

Generated wiring (`registry.register::<D>()`, CR-3) would make the
`with_rules_version`, `with_state_type` and `with_commutative` calls from the
`State` implementation, so a hand-written component rarely makes them itself.

## C ABI additions (ABI_VERSION 4, language-neutral)

### `abi.proto` changes

- **`AggregateDescriptor` gains:**
  - `string rules_version = 10;`
  - `string state_type_url = 11;` holding the fully-qualified state name, or
    empty to skip the type check (see the ambiguities).
  - `optional uint64 pack_state_callback_id = 12;` The host packs its current
    session state into a serialized `google.protobuf.Any`. This is required
    for COMMUTATIVE.
  - `optional uint64 changed_callback_id = 13;` The callback receives a
    `ChangedCall { Any before = 1; Any after = 2; }` and returns
    `ChangedUnits { repeated string units = 1; }`. Its presence turns on
    COMMUTATIVE, and it needs both `pack_state_callback_id` and
    `snapshot_callback_id`.
  - `repeated CallbackEntry command_upcasters = 14;` These are ordered rules.
    Each takes a payload of type `fq_type` and returns a serialized `Any`.
- **`ProcessManagerDescriptor` gains:** `string rules_version = 9;` and
  `string state_type_url = 10;`.
- **`CommandContextAux` gains:**
  - `uint64 expected_sequence = 4;`
  - `io.angzarr.v1.ConflictResolution conflict_resolution = 5;`
  - `bytes conflict_nonce = 6;`, set only under AGGREGATE_HANDLES;
  - `repeated io.angzarr.v1.EventPage window = 7;`, also set only under
    AGGREGATE_HANDLES.
- **The command callback's STATUS_OK output changes** from `EventBook` to a
  new message, `CommandOutput { io.angzarr.v1.EventBook events = 1; bytes
  resolved_nonce = 2; }`. A host's `resolve_conflicts` sets `resolved_nonce`.
  `STATUS_OK_EMPTY` still means no events and no acknowledgement.

The C function signatures and entry points do not change. Rules version,
snapshot refusal and the resolver all run inside `angzarr_router_dispatch`.

### How the router runs COMMUTATIVE against host-owned state

Host state lives in the binding's session; the router only ever sees `()`. For
COMMUTATIVE the router works the same single session instance in sequence:

1. Fold the pages before `E`, then pack the state as `expected`.
2. Fold the window, then pack the state as `head`.
3. Run decide.
4. Restore `head` through the snapshot loader. This undoes any mutation decide
   made.
5. Fold the emitted events, then pack the state as `after`.
6. Call `changed` twice and compute the overlap in Rust.

The overlap rule is therefore implemented once, in the router. Each language
supplies only its generated `changed`.

### A binding

At registration, the binding sends its generated rules version, state type,
`changed`, pack callback and upcasters. When a command callback runs, it builds
the handler context from `CommandContextAux`. Calling `resolve_conflicts` on
that context stores the nonce, and the binding returns it in
`CommandOutput.resolved_nonce`.

## Alternatives rejected

- **A re-entrant `angzarr_router_resolve_conflicts(host_ctx, nonce)` entry
  point** that the host calls during the callback. It mirrors the call-shaped
  API, but it re-enters the router mid-dispatch and adds thread-local
  acknowledgement state. Returning the acknowledgement as data in
  `CommandOutput` keeps every callback a pure request/response.
- **A host-side COMMUTATIVE resolver**, where the binding folds, diffs and
  decides overlap itself. That would implement the overlap rule once per
  language, which the router exists to avoid. With the proposed design the
  host provides only `changed` and state packing.
- **Requiring `S: prost::Message` for COMMUTATIVE in Rust,** to pack and diff
  bytes. `changed` is typed over the state, so `S: Clone` is enough.
- **`Conflict::resolve_conflicts(nonce)` without the window argument.** It is
  simpler, but it diverges from the spec's `resolve_conflicts(window, nonce)`,
  which every language mirrors. If the owner prefers the shorter form, it is a
  one-line change.
- **Validating snapshots inside `Rebuilder`.** The rules version is a property
  of the component, not of the fold. The dispatch tables own both checks and
  run them before rebuild.
- **Upcasting by composing an upcaster component into the same `Router`.** A
  router hosts one kind (MIXED_HANDLER_KINDS), and command upcasting belongs to
  the aggregate's own `Handle`. The chain is therefore attached to the
  aggregate.

## Spec ambiguities (owner calls)

1. **A component with no declared state type.** Should its snapshots be
   checked only against the rules version (proposed), always refused, or never
   checked? The proposal compares whatever the component declares, and
   generated wiring always declares both.
2. **Snapshot check versus an unknown type.** `Handle` validates the snapshot
   (step 1) before upcast and dispatch (steps 2 and 3), so an unknown type that
   arrives with a stale snapshot is answered `snapshot_refused` first and only
   refused on redelivery. The tests pin this literal order.
3. **Which type URL an unknown-after-upcasting command names:** the one the
   client sent (proposed and pinned) or the last one in the chain.
4. **Are facts upcast?** Step 2 says "the payload", so the proposal upcasts
   both arms.
5. **COMMUTATIVE reaching a component with no `changed`.** The coordinator
   should never deliver it (CR-2/CR-3). The proposal refuses it
   INVALID_ARGUMENT as `CONFLICT_RESOLUTION_UNSUPPORTED`. The alternative is
   ABORTED, which would treat it like STRICT.
6. **Error codes for the stale outcomes.** Should they be two codes
   (`CONFLICT_UNRESOLVED`, `CONFLICT_OVERLAP`) or one shared `STALE_COMMAND`?
   The spec fixes only the gRPC status, ABORTED.
7. **Co-resident process managers.** Each one validates its own
   `process_state` snapshot. In the merged response `snapshot_refused` is OR'd
   and the first non-empty rules version wins; that merge rule already exists.
8. **The feature files are stale.** `merge_strategy.feature` (C-0149..C-0176)
   in the coordinator contract still describes MergeStrategy, MANUAL and the
   coordinator-side Replay diff that decision 12.7 removed. C-0197 and C-0198
   are client sync-mode scenarios and don't govern the router. New scenario ids
   are needed in angzarr-project for RV-2, RV-3, NC-1, CR-1 and VC-1.
