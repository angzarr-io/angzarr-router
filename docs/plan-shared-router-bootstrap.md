---
sessions:
  - slim-pricy-power
  - witty-bony-bath
---

# Bootstrap plan: minimal Rust core + Go and Python bindings

Companion to
[decision-shared-rust-router.md](decision-shared-rust-router.md).
This plan defines the smallest set of code that proves the shared-router
design end to end in three languages, sliced into review-sized units.
Its purpose is **careful review**: every deliverable is sized to be read
line by line, the ABI is exercised by two real bindings before it
freezes, and nothing existing is destabilized — the Go engine keeps
running client-go throughout and doubles as a differential oracle.

Initial languages: **Rust (core), Go, Python**. Future bindings
(Java/Kotlin via one JVM binding, TypeScript via Node N-API, C#) start
only after this slice's review freezes the ABI.

---

## Status (angzarr-router)

- **Unit 1 — core crate**: ✅ done. `Rebuilder` + `AggregateDispatch`,
  transliterated test bank green (40 tests), `cargo-mutants` 51/51
  viable caught (≥0.95 gate met).
- **Unit 2 — FFI crate**: ✅ done. Full C-ABI + `catch_unwind` guards +
  the Rust-side ABI consumer test (21 tests).
- **Unit 3 — conformance fixture + cases**: ▶ next, not started.
- Units 4–6 (client-go / client-python / angzarr-cli): later repos,
  after the ABI-freeze review that follows unit 3.

Framework protos are consumed under the **io.angzarr** packages
(`io.angzarr.v1`); the router's own ABI protos are
`io.angzarr.router.ffi.v1`. angzarr-produced type URLs use the bare
canonical form (`/io.angzarr.v1.X`); notification/2PC recognition matches
the full FQN regardless of resolver prefix.

---

## 1. The slice: aggregate dispatch, complete; nothing else

One component kind, all of its semantics, no serving layer:

**In scope**
- `Rebuilder`: snapshot-first fold; inclusive covered-page skip;
  pageless/unknown-type entries skipped, never terminal;
  corrupt payload → `PERSISTED_EVENT_CORRUPT`; `HadPriorEvents`
  (pages OR snapshot).
- `AggregateDispatch`: envelope guards with their exact codes
  (`MISSING_COMMAND_BOOK` / `MISSING_COMMAND_PAGE` /
  `MISSING_COMMAND_PAYLOAD`); validate-before-rebuild
  (`NO_HANDLER_REGISTERED` for an unknown command, never a rebuild
  error); `CommandContext{next_sequence, had_prior_events}`;
  fill-only ext propagation and consecutive sequence stamping on
  emitted books; rejection routing — notification detection by full
  FQN (prefix-agnostic), FQ-keyed lookup, ordered multi-compensator fan-out with
  merged responses, `DelegateToFramework` (empty response) for
  undeclared rejections.
- The error model: coded errors crossing the FFI as
  `google.rpc.ErrorInfo` (reason = SCREAMING_SNAKE code, domain
  `angzarr.io`, metadata extras); unclassified handler failures →
  `UNHANDLED_HANDLER_ERROR`; Rust panics caught at every FFI entry and
  surfaced as coded failures.

**Explicitly out of scope** (and why deferring is safe)
- Saga/PM/projector/upcaster dispatch, composition, the full error
  table: more rows of the same mechanisms the slice already proves
  (registration tables, per-page callbacks, bytes-out responses,
  coded errors). They add surface, not architectural risk.
- The tonic serving layer, readiness, transport config: process
  plumbing behind the same dispatch entry point the slice exposes;
  involves no ABI design.
- Saga/PM/projector/upcaster emitter rows in angzarr-cli: the slice
  updates the aggregate emitters only (§5); the other kinds follow the
  same mechanical pattern once their dispatch lands.
- Packaging (wheels, vendored static libs): all three repos' tests
  locate the locally built artifact via the sibling-checkout pattern
  already used by `angzarr-cli`'s `validate-client`. Packaging
  pipelines are post-review work.

The aggregate path is chosen because it is the only component kind that
exercises **every** load-bearing mechanism at once: per-page applier
callbacks (the fine-grained boundary that justifies FFI), a handler
callback with framework context in and response bytes out, host-side
state that never crosses, registration tables, rejection fan-out
ordering, and the coded-error path.

---

## 2. New repo: `angzarr-io/angzarr-router`

A dedicated repo (recommendation — mirrors the `angzarr-cli` breakout;
client-rust later consumes the core crate as its native engine). Org
conventions apply: `angzarr-project` submodule for the framework
protos, `submodule.just` import, lefthook guards.

```
angzarr-router/
├── Cargo.toml                 # workspace
├── angzarr-project/           # submodule (framework protos)
├── crates/
│   ├── router/                # core crate: angzarr-router
│   │   ├── build.rs           # prost over angzarr-project framework protos
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── proto.rs       # generated framework types (books, covers…)
│   │       ├── error.rs       # CodedError {code, message, extras, grpc_code}
│   │       ├── rebuild.rs     # Rebuilder
│   │       └── aggregate.rs   # AggregateDispatch
│   └── router-ffi/            # angzarr-router-ffi: cdylib + staticlib
│       └── src/
│           ├── abi.rs         # angzarr_buf, status codes, abi_version
│           ├── registry.rs    # component descriptors → core tables
│           └── lib.rs         # extern "C" surface + panic guards
├── proto/
│   └── io/angzarr/router/ffi/v1/abi.proto   # aux payloads (below)
├── conformance/
│   ├── FIXTURE.md             # the fixture component, specified in prose
│   ├── proto/
│   │   └── test/counter/counter.proto  # the same fixture as an
│   │                          # (io.angzarr.v1.component) declaration —
│   │                          # codegen input for unit 6
│   ├── features/*.feature     # behavior in English narrative; true data
│   │                          # in Scenario-Outline Examples (run by every lang)
│   └── fixtures/*.txtpb       # orthogonal envelope skeletons only — no
│                              # test-meaningful values; step defs inject those
└── justfile                   # build / test / conformance / fmt
```

### 2.1 Core crate (review unit 1, ~500–700 lines + tests)

Rust-native API mirroring the Go engine's shapes — generic over a
host state type for native Rust use; the FFI layer instantiates it
with an opaque session handle:

```rust
pub struct Rebuilder<S> { /* factory, appliers: HashMap<FQ, ApplyFn<S>>, snapshot loader */ }
pub struct AggregateDispatch<S> { /* name, domain, rebuilder, handlers, rejections (ordered) */ }

impl<S> AggregateDispatch<S> {
    pub fn on_command(self, fq: &str, f: CommandFn<S>) -> Self;
    pub fn on_rejected(self, fq_command: &str, f: RejectionFn<S>) -> Self;  // appends: ordered fan-out
    pub fn dispatch(&self, req: &ContextualCommand) -> Result<BusinessResponse, CodedError>;
}
```

The unit-test bank is a transliteration of client-go's
`engine_test.go` + `engine_boundaries_test.go` aggregate/rebuilder
subset — that bank is mutation-hardened (0.972) and encodes the
boundary knowledge (covered-page inclusivity, `HadPriorEvents`
shapes, gap pages not terminal, fan-out order, fill-only semantics,
exact error codes). Porting the tests FIRST, red, then the
implementation, is the required order. `cargo-mutants` runs from day
one with the same triage discipline.

### 2.2 ABI aux protos

Defined in this repo under package `io.angzarr.router.ffi.v1`, versioned
with the ABI (they are internal to the boundary, not wire protocol):

```proto
message CommandContextAux { uint32 next_sequence = 1; bool had_prior_events = 2; }
message RejectionAux      { bytes notification = 1; bytes rejection = 2; CommandContextAux cctx = 3; }
// errors cross as google.rpc.ErrorInfo — no invented error proto
```

### 2.3 FFI crate (review unit 2, ~350–500 lines + tests)

The §4 ABI from the decision record, restricted to the slice:

```c
uint32_t angzarr_abi_version(void);                       // 1

typedef struct { uint8_t* data; size_t len; } angzarr_buf;
angzarr_buf* angzarr_buf_alloc(size_t);                   // router-owned; router frees

// status codes: 0 ok; <0 coded error (out carries ErrorInfo bytes)
typedef int32_t (*angzarr_cb)(void* host_ctx, uint64_t callback_id,
    const uint8_t* type_url, size_t type_url_len,
    const uint8_t* payload,  size_t payload_len,
    const uint8_t* aux,      size_t aux_len,
    angzarr_buf* out);

void* angzarr_router_new(void);
int32_t angzarr_router_register_aggregate(void* r,
    const uint8_t* descriptor, size_t descriptor_len,   // serialized descriptor proto:
    angzarr_cb cb);                                     // name, domain, command/applier/
                                                        // rejection tables with callback ids
int32_t angzarr_router_dispatch(void* r, void* host_ctx,
    const uint8_t* contextual_command, size_t len,      // ContextualCommand bytes
    angzarr_buf* out);                                  // BusinessResponse bytes | ErrorInfo bytes
void angzarr_router_free(void* r);
```

Rules under review (each carries a test):
- every entry point wraps `catch_unwind`; a panic returns a coded
  `UNHANDLED_HANDLER_ERROR` with the panic message in metadata
- router-owned buffers valid only during the callback; host fills
  `out` only via `angzarr_buf_alloc`
- one synchronous callback at a time per dispatch; dispatches on
  different host_ctx values may run concurrently
- `host_ctx` is opaque to Rust — it is where the binding parks the
  per-dispatch state object (the state-never-crosses principle made
  concrete)

The crate ships a **Rust-side ABI consumer test**: a test module that
drives the extern "C" surface through raw pointers exactly as a
foreign binding would — the ABI is proven before any binding exists.

### 2.4 The conformance fixture, behavior suite, and native harness (review unit 3)

One fixture component, specified twice from one definition: `FIXTURE.md`
prose for reviewers, and `conformance/proto/test/counter/counter.proto`
as a real `(io.angzarr.v1.component)` service declaration — the codegen
input for unit 6. The business methods are implemented in ~40 lines per
language:

> **CounterAggregate** (domain `counter`, state = an integer):
> - applier `test.counter.Increased` → state += 1
> - command `test.counter.IncreaseBy{n}` → emits n `Increased` events
>   (n > 0; n == 0 → rejection `VALUE_NOT_POSITIVE`)
> - command `test.counter.FailHard` → returns a plain error (exercises
>   `UNHANDLED_HANDLER_ERROR`)
> - rejection handlers for `test.counter.Reserve`: two registered
>   compensators appending markers (exercises ordered fan-out)
> - handler records the observed `CommandContext` (exercises
>   historical-state evidence)

**Behavior is described in Gherkin, in English narrative** — the
single cross-language contract. Cucumber is the spec because its value
is the readable behavior story; so the scenarios carry the **true test
data** (the salient values: amounts, expected counts, rejection codes)
in step text and `Scenario Outline` Examples tables, and read
top-to-bottom as behavior that survives any implementation change.

```gherkin
Scenario Outline: increasing a counter records that many events
  Given a new counter
  When the operator increases the counter by <amount>
  Then <amount> increases are recorded, continuing the sequence
  Examples: | amount | 1 | 2 | 5 |

Scenario: increasing by zero is rejected, nothing is recorded
  Given a new counter
  When the operator increases the counter by 0
  Then the command is rejected as VALUE_NOT_POSITIVE
  And no events are recorded
```

The `.txtpb` fixtures carry **only the orthogonal envelope** — the
structural boilerplate every case shares and the test is *not* about
(`ContextualCommand` → `CommandBook` → `cover{domain}` → page → `Any`
wrapping). They are value-free skeletons; the test-meaningful field is
omitted and supplied by the scenario:

```textproto
# fixtures/increase_envelope.txtpb — orthogonal scaffold only; no test data.
command {
  cover { domain: "counter" }
  pages { command { [type.googleapis.com/test.counter.IncreaseBy] {} } }
}
```

**Step definitions** (per language: cucumber-rs here, godog/behave in
the bindings) are the only per-language code: parse the skeleton, **set
the scenario's data by field** (structured — never string-templating
the textproto), dispatch, and assert the outcome. They are generic
across scenarios.

Behavior the suite covers: empty history; prior events fold; snapshot +
covered boundary (sequence == snapshot.sequence skipped, +1 applied);
gap page; corrupt payload; unknown command before rebuild; each
envelope guard; fill-only ext; sequence continuation; rejection fan-out
order; undeclared rejection → empty response; n == 0 rejection code;
FailHard → `UNHANDLED_HANDLER_ERROR`.

Unit 3 ships the suite + fixtures + the **cucumber-rs harness driving
the Rust core natively** (gate: features green against the core). Every
later subject — each binding, and the generated clients — supplies only
its step-definition layer and runs the *same* features and fixtures.
One source of truth for "correct," authored in English.

---

## 3. client-go: the Go binding (review unit 4, ~400–600 lines)

A new package, side by side with the existing engine — **nothing in
the current dispatch path changes**:

```
client-go/
└── ffirouter/
    ├── ffirouter.go        # cgo: load/link, registration API, dispatch
    ├── trampoline.go       # //export gateway: one C-visible fn → Go registry by callback_id
    ├── steps_test.go       # godog step defs: parse skeleton, set scenario data, dispatch, assert
    ├── fixture_test.go     # CounterAggregate in Go
    └── differential_test.go
```

- cgo links the locally built `angzarr-router-ffi` static/dynamic lib
  (`ANGZARR_ROUTER_LIB` env, sibling-checkout default — the
  `validate-client` pattern). Build-tagged (`//go:build ffirouter`) so
  `go test ./...` stays pure-Go until the artifact story lands.
- **Runs the same `.feature` files + `.txtpb` fixtures** from
  `../angzarr-router/conformance` via a **godog** step harness — the Go
  step defs are the only new conformance code; the behavior spec is
  shared, unchanged.
- The registration API is shaped like the engine's
  (`OnCommand(fq, thunk)`) — deliberately, since `angzarr codegen go`
  emits against it in unit 6 with minimal emitter changes. The
  hand-written fixture glue here is **transitional**: its only jobs are
  to de-risk the ABI before the generator exists and to serve as the
  differential oracle. Unit 6 deletes it.
- **`differential_test.go` is the review centerpiece**: every
  conformance scenario (and a property-style sweep of generated books)
  runs through BOTH the existing Go engine and the Rust core via FFI,
  asserting identical responses and identical error codes. The Go
  engine — mutation-hardened — is the oracle; any divergence is a
  core bug found before review ends. This test bank is also the
  retirement gate evidence for R3 later.

## 4. client-python: the Python binding (review unit 5, ~300–500 lines)

```
client-python/
└── angzarr_client/ffirouter/
    ├── __init__.py         # public registration + dispatch API
    ├── _abi.py             # cffi (ABI mode, dlopen): decls, callbacks, GIL notes
    └── tests:
        ├── test_fixture.py       # CounterAggregate in Python
        └── steps.py              # behave/pytest-bdd step defs over the same .feature + .txtpb
```

- **cffi ABI mode** for the bootstrap: pure-Python consumption of the
  same C ABI Go uses — no Rust toolchain in client-python, and the ABI
  gets validated by two genuinely different binding mechanisms
  (linked cgo vs dlopen'd cffi). `ffi.callback` handles GIL
  acquisition from router threads. A later graduation to PyO3 is an
  optimization decision, not an architecture change, because the C ABI
  must remain for the future bindings regardless.
- Runs the **same `.feature` files + `.txtpb` fixtures** via a
  **behave** (or pytest-bdd) step harness — only the Python step defs
  are new; the behavior spec is shared. The GIL-threaded dispatch
  requirement is met by exercising concurrent dispatches in a scenario.
- Decorators and the existing router package are untouched; this is
  additive.

---

## 5. angzarr-cli: generated wiring from proto (review unit 6)

The slice is not closed until the declared pipeline runs end to end:
**proto files/dirs in → generated typed seam + registration wiring out
→ binding → Rust core**. The CLI is the only supported way users wire
components, so the bootstrap proves it, not just the hand-written APIs
underneath it.

In [angzarr-cli](https://github.com/angzarr-io/angzarr-cli):

- The model layer (descriptor walking, C-0070..77 validations) is
  untouched — it is language- and runtime-neutral by design.
- The `go` and `python` emitters gain an output mode targeting the
  binding registration APIs from units 4–5: same strict typed seam
  (interface / ABC, one typed method per declared rpc, no
  `Unimplemented` embedding), and a `New<Component>Dispatch`-shaped
  constructor that assigns callback ids and registers unmarshal thunks
  with the binding instead of the Go engine's tables. The generation
  rule is unchanged: nothing that needs an `if`; semantics stay in the
  core.
- Input is the standard invocation surface — proto files/dirs or a
  descriptor set, exactly as `buf.gen.yaml` drives it today
  (`go tool angzarr-cli codegen <lang>`). The conformance fixture's
  `counter.proto` is the test input.

**Testing the generated client is the whole point of this unit** — the
generated code is what users ship, so it is a first-class conformance
subject, not the hand-written glue. The gate closes the loop in both
repos: regenerate the fixture wiring from `conformance/proto/`, delete
the transitional hand-written glue from units 4–5, and re-run **the same
`.feature` suite + `.txtpb` fixtures** (and the differential suite)
through the **generated** wiring with identical results. So the final
matrix is `cucumber × {Rust core, Go generated, Python generated}`, all
green on one English behavior spec. Any awkwardness the emitter hits
(callback-id assignment, aux unmarshaling, registration ordering) is an
ABI/API finding surfaced **before** the freeze — the emitter is the
third consumer of the registration API after the two hand-written
bindings, and the one all future users go through.

---

## 6. Review order and gates

| # | Unit | Repo | Gate |
|---|---|---|---|
| 1 | Core crate slice + transliterated test bank | angzarr-router | ✅ **done** — tests green; `cargo-mutants` 51/51 viable caught (≥ 0.95) |
| 2 | FFI crate + Rust-side ABI consumer test | angzarr-router | ✅ **done** — ABI test green; panic/ownership rules each pinned |
| 3 | Fixture + Gherkin behavior suite + cucumber-rs harness | angzarr-router | ▶ **next** — `.feature` suite green against the core natively |
| 4 | Go binding + godog harness + **differential suite** | client-go | same features green via godog; differential: zero divergence from the Go engine |
| 5 | Python binding + behave harness | client-python | same features green via behave; GIL-threaded dispatch exercised (concurrent dispatches) |
| 6 | Codegen emitters → **test the generated clients** | angzarr-cli (+ both client repos) | wiring regenerated from `conformance/proto/`; hand-written glue deleted; same features + differential green through **generated** wiring |

Each unit is one reviewable change. After unit 6: **ABI freeze review**
— the explicit decision point the decision record requires before R1
(full semantics port) and the remaining roadmap proceed. Findings from
the bindings (awkward signatures, missing aux fields) are cheap to fix
before the freeze and expensive after; that is the entire reason the
bootstrap is three languages instead of one.

## 7. What this defers, explicitly

- No change to what client-go ships today — the engine remains the
  dispatch surface until R3's parity gate.
- Emitter rows for saga/PM/projector/upcaster components; unit 6
  covers the aggregate emitters only, and the others repeat its
  pattern once their dispatch exists in the core.
- No packaging/CI artifact pipelines; sibling-checkout builds only.
- Java/Kotlin (one JVM binding), TypeScript (Node N-API), C#, and C++
  (direct C-ABI consumer, the thinnest binding) follow the frozen ABI.
