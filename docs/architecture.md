# The single client router

This library has exactly one dispatch surface: proto-declared components,
generated into strict typed seams, executed by one hand-written engine.
There is no second way to route a command or event. This document is the
architecture of record for that design — what each layer may contain, the
semantics the engine guarantees, and how the same architecture ports to
the other client languages.

## Why one router

Earlier revisions carried three parallel dispatch stacks: functional
routers (`CommandRouter`/`EventRouter`), trait-style routers behind
public interfaces, and reflection-driven OO base classes. Each
implemented page walking, state rebuild, rejection routing, and error
mapping independently — so every framework fix had to land three times,
and in practice landed once and drifted. Each stack contributed one idea
to the replacement, and all three were deleted:

- functional routers → wiring should be explicit and generatable
- trait routers → framework behavior should live once, behind a seam
- OO bases → the developer should write typed per-event methods

## The three layers

```
proto declaration            generated (angzarr codegen go)        hand-written engine
─────────────────            ──────────────────────────────        ───────────────────
service TableAggregate {     type TableAggregateHandler interface  AggregateDispatch[S]
  option (angzarr.v1           HandleCreateTable(cmd, state, cctx)   Rebuilder[S]
    .component) = {...};       ApplyTableCreated(state, event)       SagaDispatch
  rpc HandleCreateTable…       OnDealCardsRejected(…)                ProcessManagerDispatch[S]
  rpc ApplyTableCreated…     }                                       ProjectorDispatch[P]
  rpc OnDealCardsRejected…   func NewTableAggregateDispatch(h) …     Compose* / RouterBuilder
}                                                                    mapHandlerError
```

### 1. Declaration: proto services with custom options

A component is a proto service carrying `(angzarr.v1.component)`
(kind, domains, state type); each rpc declares one handled command or
event, with its input type as the registration key. Appliers carry
`(angzarr.v1.applies)`, PM source domains `(angzarr.v1.reacts)`, and
rejection handlers `(angzarr.v1.rejected)` naming the FULLY-QUALIFIED
rejected command. The declaration is the single cross-language source of
truth: every client language generates from the same service.

Declaration errors fail generation, never dispatch: missing state,
missing saga/PM/projector domains, and short (non-fully-qualified)
rejection command names are all generation-time failures (spec
C-0070..C-0075).

### 2. Generated: strict seam + table population, nothing else

`angzarr codegen go` (the shared generator in
[angzarr-cli](https://github.com/angzarr-io/angzarr-cli)) emits, per
component:

- a **strict interface** — one typed method per declared rpc. No
  `Unimplemented` embedding: a missing handler is a compile error,
  never a silent no-op.
- a **`New<Component>Dispatch` constructor** — populates the engine's
  dispatch table with unmarshal thunks that call the typed methods.

The generation rule: generate only what would be byte-identical across
components except for type names. Generated code contains **no dispatch
logic and no gRPC**. If a behavior needs an `if`, it belongs in the
engine.

### 3. Engine: framework semantics, exactly once

`engine.go`, `engine_compose.go`, `engine_grpc.go`, and `maperr.go` hold
every framework behavior. The dispatch tables are first-class entry
points — the cucumber harness and in-process composition call them
directly, with no gRPC stand-up and a single unavoidable unmarshal. The
gRPC adapters in `engine_grpc.go` are the only transport code, serving
the generic framework services.

The envelope stays `google.protobuf.Any` end to end: gRPC routes on
method path, the envelope erases payload types, and the coordinator
stays domain-blind. The dispatch table is the type_url-level complement
of that design — do not replace it with per-command rpcs without a wire
redesign.

## Engine semantics (cross-language contracts)

These behaviors are pinned by the unit suite and by mutation testing,
and every client language must match them:

| Semantic | Contract |
|---|---|
| State rebuild | Snapshot applies first; pages covered by the snapshot sequence (inclusive) never re-apply; a pageless or unknown-type entry is skipped, never terminal; a corrupt persisted payload fails the command with `PERSISTED_EVENT_CORRUPT` |
| Aggregate dispatch | Envelope and command type validate BEFORE rebuild (unknown command reports `NO_HANDLER_REGISTERED`, not a rebuild error); handlers receive `CommandContext` (next sequence, had-prior-events) |
| Fill-only stamping | The command cover's ext propagates onto emitted books, pages without headers get consecutive sequences, and saga-emitted commands inherit the source correlation ID — never overriding values the handler set itself |
| Saga dispatch | EVERY page of the source book dispatches (each page is a fresh trigger); undeclared types are skipped and the walk continues (C-0051) |
| Rejection routing | Keys are fully-qualified command type names; multiple compensators for one rejection ALL run, in registration order (C-0042); an undeclared rejection is the framework's to handle (`DelegateToFramework`) and yields an empty response by declaration |
| Process manager | The newest page of the trigger book dispatches; escalation notifications travel in `ProcessManagerHandleResponse.notification` |
| Projector | Folds every page into one projection instance; undeclared domains fold nothing (C-0032); unmatched types invoke the on-unknown hook or WARN |
| Upcaster | Chained dispatch: every matching upcaster applies in registration order, output feeding the next (C-0136); the chain stops when nothing matches the current type (C-0137) |
| Composition | Exactly one command handler owns a (domain, command) claim — duplicates fail at build (C-0010); sagas/PMs/projectors fan out in registration order; a router hosts exactly one handler kind (C-0060..C-0065) |
| Errors | One `mapHandlerError` table: rejections keep their gRPC code, `NO_HANDLER_REGISTERED` → UNIMPLEMENTED, `PERSISTED_EVENT_CORRUPT` → DATA_LOSS, other coded client errors → INVALID_ARGUMENT, unclassified → INTERNAL + `UNHANDLED_HANDLER_ERROR`. Codes ride a `google.rpc.ErrorInfo` detail (domain `angzarr.io`); clients assert on codes, never message substrings |
| Transport config | Per-instance: `ResolveTransportConfig` reads env as INPUT only (`ANGZARR_BIND_ADDRESS` > `PORT` > per-instance default); nothing writes env, so servers in one process bind their own ports and the logged port is the bound port |

## The business seam

Handlers implement the generated interface with typed methods. Business
code never sees `Any`, `Notification`, type URLs, or status codes —
decode failures, rejection plumbing, and error mapping are engine
concerns. The same seam shape is idiomatic per language: a Go interface,
a Python ABC, a Rust trait, a Java/C# interface.

## Codegen lives in the org CLI

The generator is `angzarr codegen <language>` in
[angzarr-cli](https://github.com/angzarr-io/angzarr-cli). Its model
layer (descriptor walking + ALL declaration validations) is
language-neutral and shared; emitters are per-language. A misdeclared
component therefore fails generation identically in every client
language. The CLI reads the angzarr options dynamically from the
request's own descriptor set, ships no compiled proto bindings, and can
be linked in-process — this repo's validation feature suite drives
`codegen.Generate` directly.

This repo pins the CLI as a Go `tool` directive in `go.mod`;
`buf.gen.yaml` invokes `["go", "tool", "angzarr-cli", "codegen", "go"]`.
Generated bindings are not committed — they reproduce on demand
(`just generate-proto`).

## Testing structure

Two suite hooks:

- **Unit** (`just test-unit`, also `just test`): the in-process suite —
  engine unit tests plus the cucumber client tier
  (`angzarr-project/features/client/`) driven through the real engine
  tables and real clients over loopback backends. No fakes of framework
  behavior: a scenario that cannot drive real code is honestly pending,
  never simulated.
- **Acceptance** (`just test-acceptance`): runs against the angzarr core
  deployed in kind, dialed through the real client at
  `ANGZARR_ACCEPTANCE_ENDPOINT`; targets the coordinator-contract tier.
  Coordinator/bus-side behaviors are NOT tested here in-process —
  poker-shaped functional coverage lives in the example tiers.

The engine is mutation-tested (`just mutation-test`, ooze, scoped to
`engine*.go` + `maperr.go`): coverage proves execution, mutation proves
verification. Survivors are triaged as missing tests, weak assertions,
or documented equivalents.

## Porting the architecture to another client language

The playbook, in dependency order:

1. **Spec pin**: bump that repo's `angzarr-project` submodule so the
   options and the current client feature tier are present.
2. **Engine**: port the semantics table above into one table-driven
   engine module. Existing dispatch code is raw material — restructure
   from reflection/decorator-driven registration to explicit
   `on_<kind>(fq_type, thunk)` tables.
3. **Emitter**: add the language to angzarr-cli (implement `Emitter`:
   strict seam + table constructor in that language's idiom). The
   generated output that doesn't compile yet is the engine port's
   precise TODO list.
4. **Harness**: rewrite the cucumber steps to drive the real engine
   in-process; the validation scenarios exec the shared CLI, so
   declaration validation is identical by construction.
5. **Delete** the legacy stacks once pass counts match — rewire first,
   then cut, so behavior loss is observable.
6. **Mutation-test** the engine and kill the survivors.

Python is the canonical first port; Rust, Java, and C# follow the same
sequence.
