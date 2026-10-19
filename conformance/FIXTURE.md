# Conformance fixture: CounterAggregate

The fixture is the smallest aggregate that exercises **every** load-bearing
dispatch mechanism at once. It is specified twice from one definition:

- this prose, for reviewers;
- [`proto/test/counter/counter.proto`](proto/test/counter/counter.proto) — a
  real `(io.angzarr.v1.component)` service declaration, which is the codegen
  input for unit 6.

Each language implements the business methods in ~40 lines; the behavior is
described once in [`features/`](features/) and run against every
implementation (the Rust core natively, the bindings, and the generated
clients).

## Component

**CounterAggregate** — domain `counter`, state = a single non-negative
integer (`CounterState.count`).

| Member | Type | Behavior |
|---|---|---|
| applier | `test.counter.Increased` | folds into state: `count += 1` |
| command | `test.counter.IncreaseBy { n }` | `n > 0` → emits `n` `Increased` events; `n == 0` → rejection `VALUE_NOT_POSITIVE`, nothing emitted |
| command | `test.counter.FailHard` | returns a plain (unclassified) error → `UNHANDLED_HANDLER_ERROR` |

The `IncreaseBy` handler also records the `CommandContext` it observed
(`next_sequence`, `had_prior_events`), so scenarios can assert the
historical-state evidence the framework supplies.

## What it exercises

- **Per-page appliers** — `Increased` folds during rebuild (the fine-grained
  callback boundary that justifies the FFI).
- **Handler with context in, book out** — `IncreaseBy` reads `CommandContext`
  and emits an `EventBook`.
- **Host state that never crosses** — `CounterState` lives only in the host;
  only events and the response cross the boundary.
- **Validate-before-rebuild** — an unknown command reports
  `NO_HANDLER_REGISTERED`, never a rebuild error.
- **The coded-error path** — `VALUE_NOT_POSITIVE` (business rejection) and
  `UNHANDLED_HANDLER_ERROR` (unclassified failure) both cross as
  `google.rpc.ErrorInfo`.

## The other components

`counter.proto` also declares the remaining kinds over the same
`Increased` event:

- **OrderSaga** (input domain `order`) — one `Reserve` command for
  `inventory` per source event.
- **CounterProjector** (domain `counter`) — folds a count, finishes into a
  `Projection`.
- **OrderProcessManager** (domain `order-pm`) and **AuditProcessManager**
  (domain `audit-pm`) — co-resident PMs consuming the same `counter`
  trigger, over DIFFERENT state types. The order PM emits a `Reserve`
  command and one unaddressed process event carrying an
  `OrderProcessManagerState` with the count its rebuilt state held; the
  audit PM emits one unaddressed process event carrying an
  `AuditProcessManagerState` with what its rebuilt state saw. They
  prove each PM rebuilds only its own state and that a process state
  reaches only the PM that owns it.

## The list aggregate (handling.feature)

Built by hand in the harness (no component options; the messages are at the
end of `counter.proto`): `ListState` keeps one counter per item key, with
the conflict unit `items[<key>]` (its `changed()` is hand-written to what the
generator would emit). `Bump(key)` records `Bumped(key)`; the upcaster chain
renames `BumpV1` to `Bump` and `BumpOld` to `BumpLost`, which nothing
handles. It exercises the rules version on every response, the conflict
window and nonce under AGGREGATE_HANDLES, the COMMUTATIVE resolver, and
command upcasting.

