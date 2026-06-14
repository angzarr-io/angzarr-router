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
`docs/architecture.md` is this crate's contract.

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
