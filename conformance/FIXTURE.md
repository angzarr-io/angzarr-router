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
| rejection | `test.counter.Reserve` | **two** registered compensators, each appending a marker — exercises ordered fan-out (both run, in order, events merged) |

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
- **Ordered rejection fan-out** — the two `Reserve` compensators run in
  declaration order and their events merge into one response.
- **The coded-error path** — `VALUE_NOT_POSITIVE` (business rejection) and
  `UNHANDLED_HANDLER_ERROR` (unclassified failure) both cross as
  `google.rpc.ErrorInfo`.
