---
sessions:
  - slim-pricy-power
  - witty-bony-bath
  - solid-tight-perch
  - lined-weird-race
  - gray-white-snuff
---

# Bootstrap plan: minimal Rust core + Go and Python bindings

Companion to
[decision-shared-rust-router.md](decision-shared-rust-router.md).
This plan defines the smallest set of code that proves the shared-router
design end to end in three languages, sliced into review-sized units.
Its purpose is **careful review**: every deliverable is sized to be read
line by line, the ABI is exercised by two real bindings before it
freezes, and nothing existing is destabilized — client-go keeps running
unchanged throughout. The shared conformance feature suite is the
cross-language behavior contract; no old client library is linked as a
differential oracle (§3).

Initial languages: **Rust (core), Go, Python**. Future bindings
(Java/Kotlin via one JVM binding, TypeScript via Node N-API, C#) start
only after this slice's review freezes the ABI.

---

## Status (angzarr-router)

- **Unit 1 — core crate**: ✅ done. `Rebuilder` + `AggregateDispatch`,
  transliterated test bank green (40 tests), `cargo-mutants` 51/51
  viable caught (≥0.95 gate met).
- **Unit 2 — FFI crate**: ✅ done. Full C-ABI + `catch_unwind` guards +
  the Rust-side ABI consumer test (22 tests).
- **Unit 3 — conformance suite + cucumber-rs harness**: ✅ done.
  cucumber-rs drives the core natively, parsing the `.txtpb` envelope
  skeletons and setting each scenario's data by field (17 scenarios /
  49 steps, all green). Covers: empty-history increase, prior fold +
  sequence continuation, `n==0` → VALUE_NOT_POSITIVE, FailHard →
  UNHANDLED_HANDLER_ERROR, unknown → NO_HANDLER_REGISTERED, the three
  envelope guards (MISSING_COMMAND_BOOK/PAGE/PAYLOAD), corrupt payload
  (PERSISTED_EVENT_CORRUPT), fill-only ext propagation, ordered rejection
  fan-out, undeclared rejection → empty response, snapshot + inclusive
  covered boundary, and `had_prior_events`/next_sequence evidence (the
  handler records the observed CommandContext + rebuilt count into a
  harness-owned sink, since host state never crosses).
- **Unit 4 — Go binding (`bindings/go`)**: ✅ done. cgo over `router-ffi`
  (cross-platform dynamic+rpath link), engine-shaped registration API +
  `//export` trampoline routing callbacks by id, host state never crosses
  (parked in a `cgo.Handle` session). Runs the **same** conformance
  `.feature` + `.txtpb` suite via godog (17 scenarios / 49 steps green)
  plus a 169-case binding-only property sweep. Go protobuf types generated
  with buf (managed-mode re-homed under the binding's module); google.* via
  genproto. **No old client linked** — see §3 no-old-client-linking
  decision; the shared conformance suite is the cross-language contract.
- Units 4–6 (`bindings/go` + `bindings/python` in this repo; angzarr-cli
  emitters): later, after unit 3 completes. The bindings live **in this
  repo** from the start — the home decision (§8), not a later migration;
  no old client library is linked (§3 no-old-client-linking decision). The
  **ABI-freeze review comes after unit 6**, not before the bindings — the
  whole point of doing two FFI languages in the bootstrap is that units
  4–6 exercise the ABI and surface findings while it is still cheap to
  change (§6).

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
  `angzarr.io` — the reverse-DNS error domain, distinct from the
  `io.angzarr` proto package; this follows the ErrorInfo convention,
  not a typo — metadata extras); unclassified handler failures →
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
- Packaging (wheels, vendored static libs): the `bindings/*` tests
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

A dedicated repo (recommendation — mirrors the `angzarr-cli` breakout)
that also hosts every language binding under `bindings/<lang>/` (§8); the
Rust binding consumes the core crate directly as its native engine. Org
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
├── bindings/                  # language bindings (units 4–5; §3–§4)
│   ├── go/                    # cgo over router-ffi + godog conformance
│   └── python/                # cffi over router-ffi + behave conformance
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
                                                        // rejection tables with callback ids,
                                                        // + optional snapshot-loader callback id
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
- `out` ownership is unconditional: a buffer the host allocates via
  `angzarr_buf_alloc` is router-freed whether the callback returns ok
  or a coded error — the error/partial-fill path leaks nothing
- one synchronous callback at a time per dispatch; dispatches on
  different host_ctx values may run concurrently. This is not deferred
  to a binding: the unit-2 consumer test pins it natively
  (`concurrent_dispatches_isolate_sessions` drives parallel dispatches
  across distinct sessions through the C surface and asserts per-session
  isolation), so a Send/Sync or reentrancy defect surfaces here, in
  Rust — not first through Python's GIL in unit 5
- `host_ctx` is opaque to Rust — it is where the binding parks the
  per-dispatch state object (the state-never-crosses principle made
  concrete)

The crate ships a **Rust-side ABI consumer test**: a test module that
drives the extern "C" surface through raw pointers exactly as a
foreign binding would — the ABI is proven before any binding exists.
It exercises the **full marshaling surface**, not just the happy
command path: snapshot-loader callback + covered-page skip, ordered
rejection fan-out with `RejectionAux` decode, `CommandContextAux`
(including `had_prior_events`), the `ErrorInfo` error model, the
panic / null-pointer / garbage-bytes guards, and concurrent dispatch
across distinct sessions. The hard part — the aux encode/decode and
fan-out ordering across the seam — is therefore pinned here, in Rust,
deterministically, rather than first surfacing through cgo or Python's
GIL. The marshaling channels needing this scrutiny are exactly the
boundary-invented aux payloads; the fill-only ext and sequence stamps,
by contrast, ride **in-band** inside the `ContextualCommand` /
`BusinessResponse` book bytes that already cross opaquely, so they are
covered transitively by the byte round-trip (one explicit
`cover.ext`-survives assertion on a command test closes even that).

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
across scenarios. One consequence is accepted explicitly: the
Examples-value → proto-field mapping is reimplemented in each language's
step layer (cucumber-rs / godog / behave). It is the one duplicated
piece of knowledge the design keeps per-language — small, mechanical,
and the price of a shared English spec with no shared step runtime.

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

## 3. The Go binding (review unit 4, ~400–600 lines)

A new directory **in this repo** — `bindings/go/` — built against the
core and FFI crates in the same workspace (the home decision, §8).
**client-go is untouched and not linked.** The bindings reference the
shared conformance features (which originate from the old engine's
behavior) but do not import any old client library as a live or golden
differential oracle — see "No old-client linking" below.

```
angzarr-router/bindings/go/
├── ffirouter.go        # cgo: load/link, registration API, dispatch
├── trampoline.go       # //export gateway: one C-visible fn → Go registry by callback_id
├── api.go              # pure-Go registration surface + coded-error model
├── steps_test.go       # godog step defs: parse skeleton, set scenario data, dispatch, assert
├── builders_test.go    # skeleton-parsing command/event builders (shared)
├── fixture_test.go     # CounterAggregate in Go
└── property_test.go    # broad (prior, amount) sweep vs the reference model
```

- cgo links the locally built `angzarr-router-ffi` cdylib
  (`ANGZARR_ROUTER_LIB` env, sibling-checkout default — the
  `validate-client` pattern). Build-tagged (`//go:build ffirouter`) so a
  plain build stays pure-Go until the artifact story lands. Dynamic +
  rpath keeps the link flags uniform across Linux/macOS/Windows.
- **Runs the conformance `.feature` files + `.txtpb` fixtures in-repo**
  (`../../conformance`) via a **godog** step harness — the Go step defs
  are the only new conformance code; the behavior spec is shared,
  unchanged. No cross-repo fixture reference. This shared feature suite
  is the cross-language behavior contract.
- The registration API is shaped like the engine's
  (`OnCommand(fq, thunk)`) — deliberately, since `angzarr codegen go`
  emits against it in unit 6 with minimal emitter changes. The
  hand-written fixture glue here is **transitional**: its only job is to
  de-risk the ABI before the generator exists. Unit 6 deletes it.
- **The property sweep is the breadth signal.** Because the core's unit-1
  test bank is a *transliteration* of the Go engine's, the per-scenario
  conformance replays mostly re-confirm a shared mental model. The
  independent signal lives in the freshly-authored English Gherkin spec
  and a **broad** property-style sweep of generated books (inputs neither
  author hand-picked) — `property_test.go` runs every (prior history,
  increase amount) pair across a grid and checks the core's sequence
  stamping and rejection threshold against the obvious reference model.
  Make it broad, not a token pass.

**No old-client linking (decision).** An earlier draft made
`differential_test.go` the centerpiece: replay every case through BOTH the
client-go engine and the Rust core in one test binary. That requires
linking two versions of the framework's generated code (ours,
`io.angzarr.v1`, and client-go's pre-rename `angzarr_client.proto.angzarr.v1`)
into one process. Their `options.proto` extension numbers (50100…) are
identical across the rename, so the protobuf global registry collides
irreducibly — extensions key on `(ServiceOptions, number)`, not file path,
so unlike messages they cannot be deduplicated. The only ways through are a
registration-conflict override or an out-of-process oracle, both friction
for marginal signal. **Resolution:** drop the live/golden oracle in every
language. The shared conformance feature suite is the cross-language
contract (the old engine's behavior is what authored it); each binding
proves itself by passing that suite plus a binding-only property sweep. No
binding imports an old client library. This is the parity basis R3 uses to
**replace** each engine (§8): old and new both pass the one shared suite.

## 4. The Python binding (review unit 5, ~300–500 lines)

A sibling directory **in this repo** — `bindings/python/` — over the
same C ABI (§8):

```
angzarr-router/bindings/python/
└── angzarr_router_ffi/
    ├── __init__.py         # public registration + dispatch API
    ├── _abi.py             # cffi (ABI mode, dlopen): decls, callbacks, GIL notes
    └── tests:
        ├── test_fixture.py       # CounterAggregate in Python
        └── steps.py              # behave/pytest-bdd step defs over the same .feature + .txtpb
```

- **cffi ABI mode** for the bootstrap: pure-Python consumption of the
  same C ABI Go uses — no Rust toolchain required to consume it, and the
  ABI gets validated by two genuinely different binding mechanisms
  (linked cgo vs dlopen'd cffi). `ffi.callback` handles GIL
  acquisition from router threads. A later graduation to PyO3 is an
  optimization decision, not an architecture change, because the C ABI
  must remain for the future bindings regardless.
- Runs the **same conformance `.feature` files + `.txtpb` fixtures**
  in-repo via a **behave** (or pytest-bdd) step harness — only the Python
  step defs are new; the behavior spec is shared. The GIL-threaded
  dispatch requirement is met by exercising concurrent dispatches in a
  scenario.
- **client-python is untouched and not linked**, mirroring §3's
  no-old-client-linking decision: Python references the shared conformance
  features but does not import the old client as a differential oracle (the
  same two-version registry collision applies). The shared feature suite
  plus a binding-only property sweep is the validation. This is additive
  and lands in router.

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
- The emitters target the `bindings/<lang>/` registration APIs from
  units 4–5: same strict typed seam (interface / ABC, one typed method
  per declared rpc, no `Unimplemented` embedding), and a
  `New<Component>Dispatch`-shaped constructor that assigns callback ids
  and registers unmarshal thunks with the binding instead of the Go
  engine's tables. The generation
  rule is unchanged: nothing that needs an `if`; semantics stay in the
  core. **Scope caveat — confirm before unit 6 starts:** this is an
  *added output mode* only for an emitter that already exists. The Go
  emitter does; the Python emitter may be **net-new** rather than a mode
  on an existing one. Check the angzarr-cli emitter registry
  (`codegen/generate.go`) first — if it registers only the Go emitter,
  unit 6 must build the Python emitter from scratch (the C-0070..77
  model layer is reusable, the emitter is not), which is materially
  larger than "add a mode" and is the gate that closes the whole slice.
  Size unit 6 against what is actually there, not against this sentence.
- Input is the standard invocation surface — proto files/dirs or a
  descriptor set, exactly as `buf.gen.yaml` drives it today
  (`go tool angzarr-cli codegen <lang>`). The conformance fixture's
  `counter.proto` is the test input.

**Testing the generated client is the whole point of this unit** — the
generated code is what users ship, so it is a first-class conformance
subject, not the hand-written glue. The gate closes the loop across the
two repos (angzarr-cli + angzarr-router): regenerate the `bindings/go`
and `bindings/python` fixture wiring from `conformance/proto/`, delete
the transitional hand-written glue from units 4–5, and re-run **the same
`.feature` suite + `.txtpb` fixtures** (and the property sweep)
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
| 3 | Fixture + Gherkin behavior suite + cucumber-rs harness | angzarr-router | ✅ **done** — 17 scenarios/49 steps green against the core; every slice behavior covered (guards, corrupt payload, fill-only ext, fan-out, undeclared rejection, snapshot/boundary, `had_prior_events`) |
| 4 | Go binding + godog harness + **property sweep** | angzarr-router (`bindings/go`) | ✅ **done** — 17 scenarios/49 steps green via godog against the FFI core; 169-case property sweep green; cross-platform cgo link; no old client linked |
| 5 | Python binding + behave harness | angzarr-router (`bindings/python`) | same features green via behave; binding-only property sweep; GIL-threaded dispatch exercised (concurrent dispatches); no old client linked |
| 6 | Codegen emitters → **test the generated clients** | angzarr-cli + angzarr-router | wiring regenerated from `conformance/proto/` into `bindings/*`; hand-written glue deleted; same feature suite green through **generated** wiring |

Each unit is one reviewable change — **except unit 6, which spans two
repos and cannot merge atomically.** angzarr-cli (the emitter change)
must release before angzarr-router can regenerate against it, and the
deletion gate ("hand-written glue deleted, same suite green through
generated wiring") presupposes that release. Its intra-unit sequence:
(a) angzarr-cli lands the new emitter output mode — additive, the
units 4–5 hand-written glue still present and green; (b) angzarr-router
regenerates its `bindings/go` and `bindings/python` fixture wiring,
deletes the transitional glue, re-runs the feature suite + property sweep
— one PR per binding. Each step has a green state to review against;
"unit 6" is the gate across the two repos, not a single diff. (Collapsing
units 4–6 into router + angzarr-cli, rather than spreading them over four
repos, is a direct payoff of the home decision in §8.)

After unit 6: **ABI freeze review** — the explicit decision point the
decision record requires before R1 (full semantics port) and the
remaining roadmap proceed. Findings from the bindings (awkward
signatures, missing aux fields) are cheap to fix before the freeze and
expensive after; that is the entire reason the bootstrap is three
languages instead of one. **What freezes is the dispatch-path ABI** the
slice actually exercises: the callback signature, `angzarr_buf`
ownership, the descriptor shape (including `snapshot_callback_id`), and
the `google.rpc.ErrorInfo` error model. The decision record's
serve/lifecycle surface (`_serve`, `_shutdown(drain_ms)`,
config/transport resolution) and the non-aggregate component kinds with
their aux protos are **out of this freeze and remain additive** (per
decision-record §4.5) — "freeze" here is not a claim over a surface the
bootstrap never touched.

Findings the bootstrap is expected to surface can reopen earlier units:
an emitter awkwardness in step (a) is an ABI/API finding that sends the
descriptor or callback shape back to **unit 2** before the freeze. That
round-trip is the point of three consumers — the ABI is not de-facto
settled when unit 2 lands; it settles at the post-unit-6 review.

## 7. What this defers, explicitly

- No change to what client-go ships today — its engine keeps running
  until R3 **replaces** it outright (§8); it is not modified by, nor
  linked into, the bootstrap at all (§3 no-old-client-linking decision).
- Emitter rows for saga/PM/projector/upcaster components; unit 6
  covers the aggregate emitters only, and the others repeat its
  pattern once their dispatch exists in the core.
- No packaging/CI artifact pipelines; sibling-checkout builds only.
- Java/Kotlin (one JVM binding), TypeScript (Node N-API), C#, and C++
  (direct C-ABI consumer, the thinnest binding) follow the frozen ABI.

---

## 8. Binding home: bindings live in router (ownership boundary & rationale)

The home decision, now reflected in the plan body: the bindings live in
**angzarr-router** from the start (units 4–6, §3–§5), not in per-language
client repos with a later migration. This section records the ownership
boundary and the reasoning behind that choice.

The ownership boundary the bootstrap assumes, made explicit:

- **The implementor owns** their impl-specific component proto,
  everything `angzarr codegen` generates from it, and the typed handler
  implementation — all in the implementor's repo.
- **The framework owns** the `io.angzarr.v1` protos, the angzarr-cli
  generator, the Rust core/FFI, and the per-language **binding + serving
  + registration-API** runtime.

The FFI binding (cgo for Go, cffi for Python, JNI for the JVM, …) is
**shared per-language runtime**: identical for every implementor in that
language, independent of any component. It is not implementor-specific
and not codegen output — it is hand-written runtime, the same nature as
today's `client-go/engine.go`. Being shared, it wants a single home.

**The home is angzarr-router.** A Rust core plus `bindings/<lang>/`
per language is the conventional polyglot-monorepo shape (polars,
tokenizers, pydantic-core):

```
angzarr-router/
├── crates/
│   ├── router        # core: generic native API + dispatch semantics
│   └── router-ffi    # C ABI consumed by the non-Rust bindings
└── bindings/
    ├── rust/    # native: links crates/router directly — no FFI, same shape
    ├── go/      # cgo over router-ffi — the unit-4 ffirouter, relocated
    ├── python/ # cffi over router-ffi — the unit-5 binding
    └── …       # jvm, node, c#, c++ later
# Uniform structure: every language is a bindings/<lang>/ carrying that
# language's serving/lifecycle + registration API. Rust is the same shape
# minus the FFI seam — it links the core crate instead of router-ffi.
```

Co-location directly dissolves the cross-repo coordination §6 calls out:
ABI and bindings move in lockstep, one CI runs every binding's
conformance suite + property sweep against the core, the unit-6 cross-repo
merge shrinks from four repos to two, and packaging the native lib (the
wheel-with-vendored-staticlib story §2 defers) is owned by one pipeline.

**The client repos do not survive.** Putting the bindings in router and
porting the full semantics (R1→R3) de-duplicates the *engine* — the
dispatch semantics — into the Rust
core; that is the headline win. The rest of each client is still
per-language code — the serving/lifecycle layer (`_serve`,
`_shutdown(drain_ms)`, config/transport, already **out of the ABI freeze
and additive** per §6), the idiomatic registration API the generated
code targets (`New<Component>Dispatch`, `OnCommand`, `CommandContext`),
and error/idiom mapping — but it has no reason to live as a separate
client artifact. It moves into `bindings/<lang>/` and becomes
framework-owned alongside the binding. So `client-go` / `client-python`
/ `client-rust` as standalone repos **go away**; what remains of each is
a `bindings/<lang>/` directory in router — one uniform structure across
languages. Rust keeps that same shape; it simply has no FFI seam —
`bindings/rust/` links `crates/router` natively while the other bindings
go through `router-ffi`. Its generated wiring depends on `bindings/rust/`
exactly as Go's depends on `bindings/go/` (§2).

**Sequencing and replacement.** The bindings are built in router from
unit 4 onward (§3–§5); client-go is never modified and never linked into
the bootstrap. The bootstrap proves *aggregate* dispatch only — client-go
today is far more (all component kinds, upcaster, cloudevents, identity,
destinations, serving, retry), so its full surface migrates into
`bindings/go` across the R1→R3 semantics port, and only then is the repo
replaceable. At R3, **replace** each language's engine outright — do not
keep a parallel copy running behind an indefinite parity gate. The engine
is deleted, not deprecated in place; its history lives in git if it is
ever needed. The parity basis is the shared conformance feature suite: the
old engine and the new binding both pass the one suite (it is the old
engine's behavior that authored it), so R3 is "replace once the binding is
green on the shared suite," not a bespoke live differential.

**Rust is not a bootstrap unit.** The bootstrap exists to stress the
*FFI ABI* with two genuinely different binding mechanisms (cgo vs cffi);
Rust has no FFI seam, so `bindings/rust/` (native, linking `crates/router`
directly) adds no ABI-stress signal and lands separately, whenever — the
uniform `bindings/<lang>/` structure does not imply a Rust bootstrap
unit.
