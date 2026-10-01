# Router conformance suite

The behaviour every router binding must exhibit, specified once and run
against every implementation: the Rust core natively (`crates/conformance`),
the bindings in `bindings/<language>`, and bindings that live in their
language's client repository (Python: angzarr-client-python).

| path | what it is |
|---|---|
| `features/*.feature` | the scenarios (Gherkin) |
| `proto/test/counter/counter.proto` | the fixture components, declared with the `(io.angzarr.v1.component)` options — the codegen input |
| `fixtures/*.txtpb` | canonical request skeletons (text protobuf) |
| [`FIXTURE.md`](FIXTURE.md) | what each fixture component does — the behaviour a binding's hand-written handlers implement |

## Running it from a client repository

A client repository pins this repository at a commit (a git submodule, by
convention `angzarr-router/`) and, from that one checkout:

1. **Builds the cdylib** the binding loads:

   ```bash
   cargo build --manifest-path angzarr-router/Cargo.toml -p angzarr-router-ffi --release
   # → angzarr-router/target/release/libangzarr_router_ffi.{so,dylib} / angzarr_router_ffi.dll
   ```

   The build compiles `proto/io/angzarr/router/ffi/v1/abi.proto` and the
   framework protos. Point `ANGZARR_PROJECT_PROTO` at the client repository's
   own `angzarr-project/proto` (it must be the angzarr-project revision this
   repository pins, or one with the same framework protos) instead of
   initialising this repository's nested submodule; `protoc` must be on
   `PATH` and `PROTOC_INCLUDE` must name the protobuf well-known-types
   include directory when it is not `/usr/include`.

2. **Generates the fixture wiring** with the angzarr CLI from
   `proto/test/counter/counter.proto` (plus the framework protos), using the
   client repository's own codegen templates, and the language's protobuf
   types for the same files plus `proto/io/angzarr/router/ffi/v1/abi.proto`.

3. **Implements the fixture handlers** described in `FIXTURE.md` against the
   generated `<Component>Handler` interfaces, and **runs `features/`** with the
   language's Gherkin runner, reading the feature files from the submodule in
   place (no copies).

The binding checks `angzarr_abi_version` against the ABI version it was
written for, so a pin that moves the ABI fails at load until the binding is
updated.
