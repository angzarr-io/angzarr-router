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
  prove a request reaches only the PM it names (its `component`), which
  rebuilds only its own state.

## The order aggregate and the Fulfillment process manager (handler-contract)

`crates/conformance/tests/handling.rs` binds angzarr-project's
handler-contract tier (`features/handler-contract/handling.feature`,
C-0533..C-0550, C-0565, C-0570, C-0571, C-0576, C-0577) straight from the submodule, C-0535 and C-0565 (describing the process) through
each component's `component_descriptor()`. The fixtures are built by hand over the plain messages of
`conformance/proto/orders/v1/order.proto`: `OrderState` keeps a quantity per
item (conflict unit `items[<item>]`), a shipping address (its own unit) and
unmarked notes (the wildcard `*notes`), with `changed()` hand-written to what
the generator would emit. `AddItem(item)` records `ItemAdded(item)`; the
upcaster chain renames `AddItemV1` to `AddItem`, `ItemDeliveredV1` to
`ItemDelivered` (a fact) and `AddLine` to `AddLineItem`, which nothing
handles. The Fulfillment process manager reacts to `OrderPlaced`.
