# angzarr-router — build/test/conformance for the shared client router.
# Prerequisites: Rust toolchain, protoc, the angzarr-project submodule
# initialized (or ANGZARR_PROJECT_PROTO pointing at a checkout).

TOP := `git rev-parse --show-toplevel`

# Build the workspace
build:
    cargo build --manifest-path {{TOP}}/Cargo.toml --workspace

# Run the unit test bank
test:
    cargo test --manifest-path {{TOP}}/Cargo.toml --workspace

# Mutation-test the core modules (gate: kill >= 0.95 on rebuild/aggregate).
# cargo-mutants builds in a copied tree, so the proto root must be absolute.
mutation-test:
    cd {{TOP}} && ANGZARR_PROJECT_PROTO=${ANGZARR_PROJECT_PROTO:-{{TOP}}/angzarr-project/proto} cargo mutants -f crates/router/src/rebuild.rs -f crates/router/src/aggregate.rs -f crates/router/src/error.rs -f crates/router/src/lib.rs --timeout 120

# Format + lint
fmt:
    cargo fmt --manifest-path {{TOP}}/Cargo.toml --all

lint:
    cargo clippy --manifest-path {{TOP}}/Cargo.toml --workspace --all-targets -- -D warnings
