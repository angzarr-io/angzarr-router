# Decision record: one shared Rust client router, FFI-bound to every client language

**Status**: Accepted, pending implementation. Decided 2026-06-12.

**Decision in one paragraph**: The client router — the engine that
dispatches commands/events to business handlers, rebuilds state, fans
out, and maps errors — is implemented **once, in Rust**, and embedded
in every language's component process via a C-ABI FFI binding. The
**Go engine is dropped** in favor of the Rust core, for commonality:
one implementation of the semantics, not N held equivalent by tests.
**Go remains the language of the tooling** — the `angzarr` CLI and the
code generator stay exactly where they are. The **sidecar coordinator
and the wire contract are untouched**: the coarse gRPC boundary
(whole books per RPC, `Any` envelope, domain-blind coordinator) is
preserved exactly as documented in [architecture.md](architecture.md).

This record supersedes the per-language porting playbook in
[architecture.md §"Porting the architecture"](architecture.md): no
client language ports the engine. The engine semantics table in that
document remains authoritative — it becomes the Rust core's contract
instead of a parity obligation across N implementations.

---

## 1. Context

The single-router consolidation in this repo proved the architecture:
proto-declared components → generated strict seams + table population →
one hand-written engine. The Go engine is complete, drives the full
client cucumber tier (146 scenarios, 0 failed, 0 undefined), and is
mutation-tested to a 0.972 kill rate. The question this record answers
is what happens next, for the other five client languages.

The default plan was N engine ports: each client language reimplements
the semantics table, held equivalent by the shared cucumber suite and
the shared generator validations. The estimated dominant cost per
language was the engine port (~1–1.5k lines plus a permanent
semantic-parity maintenance obligation), repeated for Python, Rust,
Java, C#, and C++.

Three observations changed the calculus:

1. **The drift machine reappears one level up.** N engine
   implementations of "fill-only correlation stamping" or "snapshot
   covered-page boundary is inclusive" are N opportunities for the
   exact class of divergence the in-repo consolidation just eliminated.
   Tests catch drift after it ships; a single implementation makes it
   structurally impossible.
2. **The org already chose this lever once.** Edition propagation moved
   from N client implementations to one canonical coordinator-side
   implementation, with the explicit rationale: one implementation
   universally applied beats N copies of the same policy. This record
   applies the same reasoning to the client-side semantics that cannot
   move coordinator-side.
3. **The Rust assets already exist.** The coordinator is Rust. The Rust
   client has a (pre-consolidation) router to consolidate anyway. The
   consolidation of client-rust stops being "port #2 of 6" and becomes
   *building the shared core*.

### Why the engine cannot simply move into the sidecar

The tempting zero-FFI version — put the engine in the (Rust) sidecar,
make every language's library a bare gRPC server — fails on call
granularity. The rebuild fold calls a business **applier per event**;
appliers are business code and stay in the host language. In-process,
that call costs nanoseconds; over even a UDS hop it costs ~50–150µs.
A 500-event rebuild becomes a 25–75ms tax on every command. The fine-
grained boundary must be in-process; FFI is the only mechanism that
shares the implementation *and* keeps the boundary in-process.

---

## 2. The decision, itemized

| # | Decision | Rationale |
|---|---|---|
| D1 | One client router implementation, in Rust, as a core crate with a Rust-native API (consumed directly by client-rust) and a C-ABI FFI crate (consumed by every other language's binding). | Commonality: the semantics table is implemented once, mutation-hardened once, fixed once. |
| D2 | **The Go engine is dropped** once the Rust core reaches parity. client-go becomes an FFI binding like every other non-Rust language. | Commonality outweighs pure-Go purity. Running N=2 engines (Go native + Rust core) preserves a smaller drift machine and a permanent double-maintenance obligation. The Go engine is not wasted work — see §7. |
| D3 | **Go remains the tooling language**: the `angzarr` CLI, the codegen model/validations, and all emitters stay in [angzarr-cli](https://github.com/angzarr-io/angzarr-cli). | Tooling is build-time, not runtime — it emits target-language source and never crosses into any process. The Go toolchain's single-static-binary distribution is exactly right for a protoc-style tool baked into build images. |
| D4 | **The sidecar coordinator and wire contract are unchanged.** The coordinator↔component boundary stays coarse: one RPC per command/trigger, whole books, `Any` envelope, domain-blind coordinator. | The coarse boundary amortizes one RPC over an entire book; nothing in this design needs the wire to know more. No coordinator release is coupled to this migration. |
| D5 | Consumer-side client wrappers (`CommandHandlerClient`, `QueryClient`, `SpeculativeClient`, `DomainClient`, builders) **remain per-language, thin**. | They are definitionally wire-protocol shaped — the protocol itself enforces commonality, and ErrorInfo codes on the wire carry the error contract. Embedding the core in arbitrary consumer contexts (UIs, scripts, other people's servers) would impose native-artifact packaging on consumers who never host a component. Reviewable; see §9 open questions. |

---

## 3. Process and boundary architecture

Topology after the change — note the process boundaries are identical
to today's:

```
┌────────────────────────────── pod ───────────────────────────────┐
│  ┌──────────────────────┐        ┌───────────────────────────┐   │
│  │ angzarr sidecar      │  gRPC  │ business component process │   │
│  │ (coordinator, Rust)  │◄──────►│                             │   │
│  │  UNCHANGED           │  UDS/  │  ┌───────────────────────┐ │   │
│  └──────────────────────┘  TCP   │  │ Rust client router    │ │   │
│        coarse boundary           │  │ (cdylib)              │ │   │
│        whole books, O(1)         │  │  · tonic server for   │ │   │
│        RPCs per command          │  │    framework services │ │   │
│                                  │  │  · engine semantics   │ │   │
│                                  │  │  · dispatch tables    │ │   │
│                                  │  └──────────┬────────────┘ │   │
│                                  │     C-ABI   │  callbacks,  │   │
│                                  │     fine    │  O(pages)    │   │
│                                  │  ┌──────────▼────────────┐ │   │
│                                  │  │ language binding      │ │   │
│                                  │  │ (PyO3 / cgo / JNI /   │ │   │
│                                  │  │  P/Invoke)            │ │   │
│                                  │  ├───────────────────────┤ │   │
│                                  │  │ generated typed seam  │ │   │
│                                  │  │ (angzarr codegen <L>) │ │   │
│                                  │  ├───────────────────────┤ │   │
│                                  │  │ business handlers     │ │   │
│                                  │  │ (host language)       │ │   │
│                                  │  └───────────────────────┘ │   │
│                                  └─────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────┘
```

### The granularity rule (load-bearing)

| Boundary | Mechanism | Granularity | Crossing cost | Crossings per command |
|---|---|---|---|---|
| coordinator ↔ router | gRPC (unchanged) | whole books | ~50–150µs | O(1) |
| router ↔ business code | C-ABI callback | per page / per handler | ~1–5µs incl. GIL/JNI attach | O(pages) |

Every placement question reduces to this rule:

- A semantic that needs a **per-event or per-handler business
  callback** (rebuild fold, command/event/trigger dispatch, rejection
  compensation) lives in the **router** — shared Rust, in-process.
- A semantic that operates on **whole books and covers with no
  business code in the loop** is a candidate for the **coordinator**
  (the edition-propagation precedent) — written once *and* requiring
  no binding at all.

The wire must never become finer (per-event RPCs), and the FFI must
never become coarser (serializing state across it).

---

## 4. The FFI ABI

This is the contract reviewers should read most carefully. Design
principles first:

- **Business types never cross.** The router sees business payloads as
  opaque bytes plus a fully-qualified type name. Only the host language
  unmarshals business messages, using its own generated protobuf
  classes.
- **Framework types never cross as objects.** Books, covers, pages —
  the router parses and assembles these in Rust, where they are native.
  When the host must return framework data (an emitted `EventBook`), it
  returns serialized proto bytes; proto is the interchange format on
  both boundaries.
- **State never crosses at all.** Aggregate/PM state is a host-language
  object held by the host for the duration of one dispatch. The router
  orchestrates *when* appliers and handlers run; it never sees what
  they fold. No state serialization, no cross-runtime handles, no
  ownership transfer.

### 4.1 Lifecycle surface

```
angzarr_router_new(config) -> *Router
    config: serialized proto or flat struct — transport (addr/UDS),
    service name, domain, probe settings. Env-as-input resolution
    (ANGZARR_BIND_ADDRESS > PORT > default) happens in the router,
    matching the per-instance transport contract.

angzarr_router_register_component(r, descriptor) -> status
    descriptor: kind (aggregate|saga|pm|projector|upcaster), name,
    input/output domains, and the registration tables:
      commands[]:   (fq_type, callback_id)
      events[]:     (source_domain?, fq_type, callback_id)   // PM carries source
      appliers[]:   (fq_type, callback_id)
      rejections[]: (fq_command_type, callback_id[])          // ordered fan-out
      on_unknown?:  callback_id                               // projector hook
    Registration is where build-time validation fires (duplicate
    claims, mixed kinds, empty routers — the C-0060..65 codes), so a
    misconfiguration fails at startup, before serving.

angzarr_router_serve(r) -> status        // blocks: tonic serve + signal handling
angzarr_router_shutdown(r, drain_ms)     // NOT_SERVING → drain → graceful stop
angzarr_router_free(r)
```

A non-serving dispatch entry point is also exposed so harnesses and
in-process composition can drive tables directly (the cucumber suites
depend on this):

```
angzarr_router_dispatch(r, kind, request_bytes, &response_buf) -> status
    request/response are the framework service messages
    (ContextualCommand → BusinessResponse, SagaHandleRequest →
    SagaResponse, …) as proto bytes.
```

### 4.2 Callback contract

All callbacks share one C signature; `callback_id` selects the host
thunk (generated wiring assigns ids at registration):

```
typedef int32_t (*angzarr_cb)(
    void*        host_ctx,      // binding-owned; carries the per-dispatch
                                // session (where host-side state lives)
    uint64_t     callback_id,
    const uint8_t* type_url,  size_t type_url_len,
    const uint8_t* payload,   size_t payload_len,   // business message bytes
    const uint8_t* aux,       size_t aux_len,       // serialized context proto
    angzarr_buf* out);                               // host-filled response
```

`aux` carries the per-call framework context as a small serialized
proto, by callback kind:

| Kind | aux contents | out contents |
|---|---|---|
| applier | (empty) | (empty — host mutates session state) |
| command handler | `CommandContext{next_sequence, had_prior_events}` | emitted `EventBook` bytes |
| saga handler | `destination_sequences` map | `(CommandBook[], EventBook[])` wrapper bytes |
| PM handler | `destination_sequences` map | `ProcessManagerHandleResponse` bytes |
| projector fold | (empty) | (empty — host mutates session projection) |
| projector finish | full `EventBook` bytes | `Projection` bytes |
| rejection | `Notification` + `RejectionNotification` bytes (+ `CommandContext` for aggregates) | kind-appropriate response bytes |
| upcaster | (empty) | transformed event `Any` bytes |

The host-side `Destinations` helper (sequence lookup + `StampCommand`)
is reconstructed in the binding from the `destination_sequences` map —
it is convenience sugar, not shared state.

Return codes: `0` success; negative = coded business error, in which
case `out` carries a serialized `ErrorInfo`-shaped proto
(`{reason: SCREAMING_SNAKE_CODE, domain: "angzarr.io", metadata}`).
The router maps it through the single error table — the same
`mapHandlerError` semantics, now in exactly one place.

### 4.3 Memory ownership

Symmetric and copy-at-the-boundary; no shared ownership:

- Router → host: `type_url`/`payload`/`aux` buffers are router-owned
  and valid **only for the duration of the callback**. Hosts that need
  retention copy (in practice they immediately unmarshal, which copies).
- Host → router: `out` is filled via `angzarr_buf_alloc(size)`
  (router-provided allocator) so the router frees what it allocated.
  Bindings never `free` router memory; the router never frees host
  memory.

### 4.4 Threading, reentrancy, panics

- Callbacks are invoked **synchronously** from router worker threads
  (tokio blocking pool). Bindings own runtime-entry: PyO3 acquires the
  GIL per callback; JNI attaches the thread once and caches; cgo
  callbacks are natively safe. Handlers are written synchronously in
  every language (as they are today); async hosts adapt in the binding
  if ever needed.
- Per-dispatch ordering guarantees are preserved (appliers in page
  order, then the handler; fan-out in registration order). Distinct
  dispatches may run concurrently — same as today's engine, where
  per-dispatch state isolation comes from fresh state per call.
- Callbacks must not re-enter `angzarr_router_dispatch` on the same
  session (documented; debug builds assert).
- **Panic policy**: every FFI entry point wraps `catch_unwind`. A Rust
  panic surfaces as `UNHANDLED_HANDLER_ERROR`-class coded failure with
  the panic message in metadata — never an abort, never an unwind
  across the boundary. Host exceptions are caught by the binding
  trampoline and converted to the coded-error return before crossing
  back.

### 4.5 ABI versioning

`angzarr_abi_version() -> u32`, checked by every binding at load; the
FFI crate is semver'd independently of the core crate. Additions are
new functions/callback kinds (minor); signature changes are major and
require coordinated binding releases. Capability probing
(`angzarr_has_capability(name)`) covers optional features so bindings
can degrade gracefully.

---

## 5. What lives where, after

### Rust core (workspace in the client-rust repo)

- `angzarr-router` (crate, Rust-native API): the entire semantics
  table from [architecture.md](architecture.md) — Rebuilder with the
  inclusive covered-page boundary and `PERSISTED_EVENT_CORRUPT`;
  validate-before-rebuild aggregate dispatch with `CommandContext`;
  fill-only ext/sequence/correlation stamping; every-page saga walk
  with skip-never-terminates; FQ-keyed ordered rejection fan-out with
  `DelegateToFramework` default; newest-page PM trigger and
  escalation notification; projector domain filter and on-unknown;
  chained upcaster dispatch; the composition layer and its build-time
  codes; the single error-mapping table with `ErrorInfo`; per-instance
  transport resolution; readiness/health/drain. Plus the tonic
  adapters for the framework services.
- `angzarr-router-ffi` (crate, `cdylib` + `staticlib`): the §4 ABI
  over the core. Nothing else — no semantics in the FFI layer.
- client-rust's public client library consumes `angzarr-router`
  natively; Rust business code implements generated traits with no FFI
  anywhere.

### Per language (Python, Go, Java, C#, C++)

- **Binding** (~500–1k lines, written once, near-frozen): library
  loading, the registration API the generated wiring targets, callback
  trampolines (runtime entry, exception→coded-error conversion),
  buffer marshaling, error translation into the language's idiomatic
  error type carrying `Code`/`Extras`.
- **Generated wiring** (`angzarr codegen <lang>`): the strict typed
  seam (ABC / interface / trait) + registration glue that assigns
  callback ids and unmarshals payload bytes into typed messages. Same
  generation rule as ever: nothing that needs an `if`.
- **Thin consumer clients** (per D5) and the language's protobuf
  classes.

### Tooling (unchanged home)

- `angzarr-cli` (Go): codegen model + validations + per-language
  emitters. Emitters now target each binding's registration API — the
  model layer and the validation contract are untouched by this
  decision.

---

## 6. Testing strategy

- **The core owns the semantics proof.** The client cucumber tier
  (`features/client/`) is driven natively in Rust against the core's
  tables — the Go harness's structure (real engine in-process, real
  clients over loopback backends, no simulations) is the template.
  `cargo-mutants` on the engine with the survivor-triage discipline;
  the bar is the Go engine's 0.972, and the Go engine's mutation-
  hardened test bank (boundary tests, walk-continuation tests,
  fan-out merge tests) transliterates into the Rust suite as the
  starting corpus.
- **Each binding proves the trampoline, not the semantics.** Every
  language still runs the full client tier through its binding — green
  means "registration, marshaling, error translation, and threading
  are correct," and a failure is a binding bug by construction, since
  the semantics underneath are the already-proven core. This is the
  decisive maintenance shift: the suites stop being drift detectors
  and become integration checks.
- **Validation scenarios** (C-0070..77) exec the shared CLI everywhere
  — unchanged, already language-independent.
- **The acceptance hook** (against the kind-deployed coordinator) is
  unaffected; the coordinator contract tier remains its target.

---

## 7. Migration roadmap

Rewire-then-delete discipline throughout: nothing is removed until the
replacement passes the identical suite.

- **R0 — Core skeleton**: workspace crates in client-rust; transplant
  the architecture doc's semantics table as the contract; set up
  cargo-mutants and the cucumber harness scaffolding.
- **R1 — Engine port (the bulk)**: implement the semantics table TDD,
  transliterating the Go engine's unit-test bank first (it encodes the
  mutation-hardened boundary knowledge: covered-page inclusivity,
  HadPriorEvents shapes, skip-vs-terminate walks, fan-out order,
  first-notification-wins). client-rust's existing `router/` +
  `dispatch` code is raw material; `angzarr-macros` is the legacy
  stack scheduled for its P4 deletion. Gate: client tier numbers match
  Go's (145+/146, 0 failed, 0 undefined) natively in Rust, mutation
  kill ≥ 0.97.
- **R2 — FFI + first binding (Python)**: `angzarr-router-ffi`; PyO3
  binding (maturin abi3 wheels); `codegen python` emitter in
  angzarr-cli; client-python spec-pin bump, harness rewrite to drive
  the binding, decorator-stack deletion. Python is the canonical
  binding: dynamic language, GIL, the hardest trampoline — if the ABI
  is comfortable here, it is comfortable everywhere.
- **R3 — Go binding, Go engine retirement**: cgo binding in client-go;
  generated wiring re-emitted against the binding API (emitter change
  in angzarr-cli); the suite runs through the binding until pass
  counts match exactly; then `engine.go`/`engine_compose.go`/
  `engine_grpc.go`/`maperr.go` dispatch internals are deleted. The Go
  engine's tests outlive it twice: transliterated into the Rust core
  (R1) and retained where they pin binding-visible behavior. Consumer
  impact documented prominently: building a Go component now requires
  cgo + a platform artifact (prebuilt static libs shipped per
  platform; `CGO_ENABLED=1`).
- **R4 — Future bindings: Java/Kotlin, TypeScript, C#, C++.** One JVM
  binding (JNI or Panama) serves both Java and Kotlin; TypeScript
  binds via a Node N-API addon (components are server processes, so
  Node is the runtime that matters); C# via P/Invoke; C++ consumes the
  C ABI directly (or links the core statically) — the thinnest binding
  of the set. None of these start before the ABI has survived the
  R2/R3 bindings unchanged.
- **R5 — Coordinator-side semantics audit**: with the core in place,
  walk the semantics table asking "whole-book, no business callback?"
  and migrate qualifying rows coordinator-side (the §3 rule),
  shrinking the core itself.

Sequencing note: R2 before R3 deliberately — the Go engine keeps
client-go fully functional and acts as the live cross-check against
the core during the riskiest phase, then retires.

**Bootstrap scope.** The roadmap above is entered through a minimal,
review-gated vertical slice across exactly three languages — the Rust
core slice with Go and Python bindings — specified in detail in
[plan-shared-router-bootstrap.md](plan-shared-router-bootstrap.md).
Nothing beyond the slice starts until that code has been reviewed and
the ABI frozen.

---

## 8. Consequences

**Gained**
- The semantics table is implemented once, mutation-hardened once,
  and fixed once; cross-language drift in dispatch behavior becomes
  structurally impossible rather than test-suppressed.
- Four engine ports (Python, Java, C#, C++) never happen; the Rust
  consolidation was owed anyway.
- Cucumber suites downgrade from drift detectors to binding checks —
  a permanent reduction in what "keeping six languages correct" costs.
- The core can share crates with the coordinator (framework types,
  sequencing arithmetic), tightening client/coordinator agreement.

**Paid**
- A native-artifact pipeline: (5 languages) × (linux/macos/windows) ×
  (amd64/arm64) built, signed, and shipped per release; build images
  updated; per-language packaging (wheels, jars, NuGet, Go static
  libs).
- client-go consumers lose pure-Go builds (cgo). Accepted explicitly
  in D2 — commonality over purity.
- Debugging spans a language boundary; mitigated by the panic policy,
  coded errors with metadata, and structured logging on both sides.
- The Go engine — complete and hardened — is retired at R3. Its value
  is realized as the executable specification for R1 and the test
  corpus the core inherits, not discarded.

**Risks and mitigations**
- *GIL/attach deadlocks or fork-unsafety* (Python prefork servers):
  components are dedicated server processes (the run entry point
  blocks); document "no fork after init"; CI exercises signal-driven
  shutdown per binding.
- *ABI churn during R1/R2*: the ABI is reviewed with this record and
  frozen before R2 ships; additions only, per §4.5.
- *Artifact supply chain*: artifacts built in org CI only, checksummed
  and signed; bindings verify at load where the ecosystem supports it.
- *Single point of failure*: a core bug now ships to every language at
  once — which is also true of every coordinator bug today; the
  mutation bar and the N-suite binding checks are the compensating
  controls, and one fix also ships everywhere at once.

---

## 9. Open questions for review

1. **D5 scope** — thin per-language consumer clients: confirm, or pull
   the consumer-side error-extraction/retry into the core too for
   maximum commonality (at the cost of native artifacts for
   non-component consumers)?
2. **Core home** — workspace crates inside client-rust (proposed:
   one source of truth, Rust users consume directly) vs a separate
   `angzarr-router` repo vs adjacent to the coordinator's crates?
3. **Go artifact distribution** — vendored prebuilt static libs per
   platform inside the module vs a separate fetch step in build
   tooling? (Affects `go get` ergonomics directly.)
4. **CloudEvents runners** — bind through the core's adapters or
   remain a thin per-language adapter over the dispatch entry point?
5. **Upcaster placement** — upcasters are stateless byte transforms;
   they fit the callback model trivially, but are also the one
   component kind simple enough to leave per-language if binding
   surface area should shrink. Proposed: in the core, for uniformity.
