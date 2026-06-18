# angzarr-router

The shared angzarr client router: the engine that dispatches
commands/events to business handlers, rebuilds state, fans out, and maps
errors — implemented **once, in Rust**, consumed natively by client-rust
and through a C-ABI FFI crate by every other client language.

Decision record and bootstrap plan live in client-go:
`docs/decision-shared-rust-router.md` and
`docs/plan-shared-router-bootstrap.md` on the
[angzarr-client-go](https://github.com/angzarr-io/angzarr-client-go)
repository. The engine semantics table in client-go's
`docs/architecture.md` is this crate's contract. The org-wide decision is
recorded as `docs/adr/0001-shared-rust-router-via-ffi.md` in
[angzarr-project](https://github.com/angzarr-io/angzarr-project).

## Why this exists: code size & complexity

The dispatch engine — command/event routing, state rebuild, error→gRPC
mapping, sequence stamping, compensation/rejection fan-out — used to be
hand-written and maintained **once per language**. Every client was a
line-for-line transliteration of the same subtle semantics. This crate
implements that engine **once** and shares it over the C ABI; each
language keeps only a thin marshaling binding (and its own transport,
which the router never touches).

Measured prod LOC, dispatch layer only (transport / coordinator excluded —
they stay per-language either way):

| | Old (per language) | New |
|---|---|---|
| Semantic core | ~4,000, owned alone | **2,115**, shared (÷ N languages) |
| Marshaling / binding | (folded into the above) | ~950, mechanical, partly generated |

Per language the dispatch layer drops **~4,000 → ~1,300** LOC, and the
*semantic* share drops ~4,000 → ~350 (amortized over six languages).
Across six languages, **~24,000 → ~7,800** total — and the bug-prone
invariants (fill-only correlation, last-page-only PM trigger,
undeclared-is-silent-not-error, exact type-URL match, ordered
compensation) are implemented, mutation-tested to ~1.0 kill, and proven by
**one** behavioral conformance suite run against every language — not
re-derived and re-tested six times. A fix lands once, everywhere.

(Figures are prod-LOC proxies with a hand-drawn dispatch/transport
boundary; the core currently covers three of four component kinds —
process-manager adds ~250 core + ~120 FFI. The shared test bank — core +
FFI + conformance, ~2,700 LOC — is likewise written once.)

## Layout

```
crates/
└── router/          # core crate: Rust-native API (Rebuilder, AggregateDispatch)
```

The FFI crate (`crates/router-ffi`) and the conformance fixture
(`conformance/`) land in the bootstrap's later review units.

## Protos

The framework protos come from the repo-local `angzarr-project/`
submodule (the org convention); `ANGZARR_PROJECT_PROTO` overrides it
when set. The `mutation-test` target passes the submodule's absolute
path through that env var so cargo-mutants' copied trees build.

## Development

```bash
just build          # cargo build --workspace
just test           # the transliterated engine test bank
just mutation-test  # gate: kill >= 0.95 on rebuild.rs / aggregate.rs
just lint           # clippy -D warnings
```

The unit-test bank is a transliteration of client-go's mutation-hardened
`engine_test.go` / `engine_boundaries_test.go` aggregate subset (kill
0.972) — it encodes the boundary knowledge: covered-page inclusivity,
`had_prior_events` shapes, gap pages never terminal, fan-out order,
fill-only stamping, exact error codes.
