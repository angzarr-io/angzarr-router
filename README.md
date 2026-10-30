# angzarr-router

The angzarr router for Rust: the part of a client library that calls
business code. It is the reference router; Python has a native router of
its own (decision 14.42), and every router, in every language, is held to
the same contract, angzarr-project's handler-contract tier
(`features/handler-contract/`).

The router keeps dispatch, and only dispatch. For one component (an
aggregate, a process manager, a policy or a projector) it:

- routes an input by its type URL to the business code registered for it,
  refusing a type nothing handles (`ROUTER_REASON_UNKNOWN_COMMAND_TYPE`,
  `ROUTER_REASON_NO_FACT_HANDLER`, `ROUTER_REASON_NO_REFUSAL_HANDLER`);
- folds the starting state and every page it is given;
- runs decide (a command) or ingest (a fact), and answers the events, the
  state after them and the build's rules version;
- reports a stale command's conflicts: the nonce business code settled
  with (AGGREGATE_HANDLES), or the window's and the command's conflict
  units (COMMUTATIVE);
- runs a policy's handlers once per page and answers their commands
  grouped by the page that triggered them, and runs its translating
  callback for a refusal, or answers that it has none;
- runs a projector;
- turns whatever escapes business code into a reason key and its args.

Everything else is the coordinator's (core's coordinator, and the
angzarr-embedded kernel): upcasting, through the component's upcaster
sidecar (14.39); loading snapshots and dropping the pages they cover, and
deciding when to write one (14.40); admission (deduplication, the refusal
fact's stream check, a process manager's start rule); validating the
descriptor angzarr-cli generates; the reason catalog and classifying a
status; checking and stamping a policy's commands; building and addressing
refusal facts; envelope guards; and judging a conflict report (14.42).

The router has no transport: the host serves the coordinator's gRPC
services and hands the framework messages to it. There is no C ABI; the
router-ffi crate and the hand-written Go, Java, C#, C++ and TypeScript
bindings are retired (git history keeps them).

See [docs/architecture.md](docs/architecture.md) for the API and the
semantics it guarantees.

## Layout

```
crates/
├── router/        # the router: Rebuilder, DomainDispatch (aggregate or PM),
│                  # PolicyDispatch, ProjectorDispatch, Router, CodedError
└── conformance/   # cucumber-rs harness: the handler-contract tier against the router
tools/archlint/    # the architecture linter (archlint.toml)
```

## Protos

The framework protos come from the repo-local `angzarr-project/`
submodule (the org convention); `ANGZARR_PROJECT_PROTO` overrides it when
set. The io.angzarr.v1 wire types are angzarr-project's `rust/angzarr-proto`
crate at the submodule's commit (re-exported as `angzarr_router::pb`);
`crates/router/build.rs` generates only the service-proto messages that
crate does not compile (`CommandHandlerResponse`, `PolicyResponse`,
`Refusal`, `OnRefusedResponse`), against its types. The reason keys are
reasons.proto's `RouterReason` (decision 14.44), each answered as its full
value name (`as_str_name()`, e.g. `ROUTER_REASON_DUPLICATE_COMPONENT`). The
`mutation-test` target passes the submodule's absolute path through
`ANGZARR_PROJECT_PROTO` so cargo-mutants' copied trees build.

## Development

```bash
just build              # cargo build --workspace
just test               # the unit bank and the handler-contract tier
just mutation-test      # cargo-mutants over the router and archlint, 100%
just mutation-push      # by hand: the same, over what a push adds (stdin: git's pushed refs)
just push-check origin  # pre-push: every test of each pushed commit; red refused on main only
just lint               # clippy -D warnings
just archlint           # module-level layer rules (archlint.toml)
just fmt-check          # rustfmt, no rewrite
```

Every recipe runs in the org's pinned Rust toolchain image (see
`justfile`).
