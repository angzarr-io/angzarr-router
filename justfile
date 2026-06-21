# angzarr-router development commands.
#
# Container Overlay Pattern (same as angzarr core):
#   1. `justfile` (this file) runs on the host and delegates into the org's
#      pinned toolchain image.
#   2. `justfile.container` is mounted over this file inside the container and
#      holds the real recipes (cargo, and buf with the pinned plugins).
#   3. DEVCONTAINER=true short-circuits to run recipes directly — no nesting.
#
# So `just buf-lint` / `just build` produce identical results on the host,
# from inside the coordinator container, and inside a devcontainer.
set shell := ["bash", "-c"]

# Submodule-protection recipes (install-submodule-hooks,
# check-submodules-clean). Source of truth: angzarr-project/submodule.just.
import? 'angzarr-project/submodule.just'

TOP := `git rev-parse --show-toplevel`
# Pinned org toolchain images, one per language (the org convention). The
# Rust recipes (and the router-ffi cdylib) build in the rust image; the Go
# binding builds/tests in the go image — the same `angzarr-go` image
# client-go uses. The cdylib is the ABI boundary and is carried forward
# between the two (built once in rust, linked in go via the shared target/
# mount), so no single all-languages image is required.
# Override either with the matching env var to pin a git-SHA tag.
ROUTER_IMAGE := env_var_or_default("ANGZARR_ROUTER_IMAGE", "ghcr.io/angzarr-io/angzarr-rust:latest")
ROUTER_GO_IMAGE := env_var_or_default("ANGZARR_ROUTER_GO_IMAGE", "ghcr.io/angzarr-io/angzarr-go:latest")
ROUTER_PYTHON_IMAGE := env_var_or_default("ANGZARR_ROUTER_PYTHON_IMAGE", "ghcr.io/angzarr-io/angzarr-python:latest")
ROUTER_JAVA_IMAGE := env_var_or_default("ANGZARR_ROUTER_JAVA_IMAGE", "ghcr.io/angzarr-io/angzarr-java:latest")
ROUTER_CSHARP_IMAGE := env_var_or_default("ANGZARR_ROUTER_CSHARP_IMAGE", "ghcr.io/angzarr-io/angzarr-csharp:latest")
ROUTER_CPP_IMAGE := env_var_or_default("ANGZARR_ROUTER_CPP_IMAGE", "ghcr.io/angzarr-io/angzarr-cpp:latest")
ROUTER_TYPESCRIPT_IMAGE := env_var_or_default("ANGZARR_ROUTER_TYPESCRIPT_IMAGE", "ghcr.io/angzarr-io/angzarr-typescript:latest")
# Container runtime: docker (rootless or rootful). Empty inside a container.
CONTAINER_CMD := `command -v docker 2>/dev/null || echo ""`
# `-u $(id -u):$(id -g)` is right for ROOTFUL docker (bind-mount files get the
# host UID). With ROOTLESS docker that is WRONG: the userns maps
# container-root → host UID, so -u $(id -u) remaps onto an unowned subuid and
# breaks bind-mount writes. Force -u 0:0 instead of relying on the image's
# default user — images that set a non-root USER (the python image runs as
# `angzarr`) otherwise land on a subuid that cannot write the mount (and trips
# git's dubious-ownership guard). Running as container-root maps to the host UID.
CONTAINER_USER_ARG := if `docker info 2>/dev/null | grep -q rootless && echo yes || echo no` == "yes" { "-u 0:0" } else { "-u $(id -u):$(id -g)" }
CONTAINER_RUN := CONTAINER_CMD + " run --rm " + CONTAINER_USER_ARG

# Delegate a container-side recipe: run directly inside a devcontainer,
# otherwise in the pinned image with justfile.container overlaid as justfile.
[private]
_container +ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "${DEVCONTAINER:-}" = "true" ]; then
        just --justfile "{{TOP}}/justfile.container" {{ARGS}}
    else
        {{CONTAINER_RUN}} --network=host \
            -v "{{TOP}}:/workspace:Z" \
            -v "{{TOP}}/justfile.container:/workspace/justfile:ro" \
            -w /workspace \
            -e CARGO_HOME=/workspace/.cargo-container \
            -e ANGZARR_PROJECT_PROTO=/workspace/angzarr-project/proto \
            "{{ROUTER_IMAGE}}" just {{ARGS}}
    fi

# Same delegation, into the Go toolchain image (go + cgo + buf +
# protoc-gen-go). The router-ffi cdylib the binding links is built first in
# the rust image and carried forward via the shared target/ mount.
[private]
_go_container +ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "${DEVCONTAINER:-}" = "true" ]; then
        just --justfile "{{TOP}}/justfile.container" {{ARGS}}
    else
        {{CONTAINER_RUN}} --network=host \
            -v "{{TOP}}:/workspace:Z" \
            -v "{{TOP}}/justfile.container:/workspace/justfile:ro" \
            -w /workspace \
            -e ANGZARR_PROJECT_PROTO=/workspace/angzarr-project/proto \
            "{{ROUTER_GO_IMAGE}}" just {{ARGS}}
    fi

# Same delegation, into the Python toolchain image (python + uv + buf +
# grpcio-tools). The router-ffi cdylib the binding dlopens is built first in
# the rust image and carried forward via the shared target/ mount.
[private]
_py_container +ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "${DEVCONTAINER:-}" = "true" ]; then
        just --justfile "{{TOP}}/justfile.container" {{ARGS}}
    else
        {{CONTAINER_RUN}} --network=host \
            -v "{{TOP}}:/workspace:Z" \
            -v "{{TOP}}/justfile.container:/workspace/justfile:ro" \
            -w /workspace \
            -e ANGZARR_PROJECT_PROTO=/workspace/angzarr-project/proto \
            "{{ROUTER_PYTHON_IMAGE}}" just {{ARGS}}
    fi

# Same delegation, into the Java toolchain image (JDK 25 + Gradle + buf +
# angzarr). The router-ffi cdylib the binding loads via Panama/FFM is built
# first in the rust image and carried forward via the shared target/ mount.
[private]
_java_container +ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "${DEVCONTAINER:-}" = "true" ]; then
        just --justfile "{{TOP}}/justfile.container" {{ARGS}}
    else
        {{CONTAINER_RUN}} --network=host \
            -v "{{TOP}}:/workspace:Z" \
            -v "{{TOP}}/justfile.container:/workspace/justfile:ro" \
            -w /workspace \
            -e ANGZARR_PROJECT_PROTO=/workspace/angzarr-project/proto \
            "{{ROUTER_JAVA_IMAGE}}" just {{ARGS}}
    fi

# Same delegation, into the C# toolchain image (.NET SDK + buf + angzarr). The
# router-ffi cdylib the binding loads via P/Invoke is built first in the rust
# image and carried forward via the shared target/ mount.
[private]
_csharp_container +ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "${DEVCONTAINER:-}" = "true" ]; then
        just --justfile "{{TOP}}/justfile.container" {{ARGS}}
    else
        {{CONTAINER_RUN}} --network=host \
            -v "{{TOP}}:/workspace:Z" \
            -v "{{TOP}}/justfile.container:/workspace/justfile:ro" \
            -w /workspace \
            -e ANGZARR_PROJECT_PROTO=/workspace/angzarr-project/proto \
            "{{ROUTER_CSHARP_IMAGE}}" just {{ARGS}}
    fi

# Same delegation, into the C++ toolchain image (clang/cmake + buf + angzarr).
# The router-ffi STATICLIB the binding links directly is built first in the
# rust image and carried forward via the shared target/ mount.
[private]
_cpp_container +ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "${DEVCONTAINER:-}" = "true" ]; then
        just --justfile "{{TOP}}/justfile.container" {{ARGS}}
    else
        {{CONTAINER_RUN}} --network=host \
            -v "{{TOP}}:/workspace:Z" \
            -v "{{TOP}}/justfile.container:/workspace/justfile:ro" \
            -w /workspace \
            -e ANGZARR_PROJECT_PROTO=/workspace/angzarr-project/proto \
            "{{ROUTER_CPP_IMAGE}}" just {{ARGS}}
    fi

# Same delegation, into the TypeScript toolchain image (Node + buf + angzarr).
# The router-ffi cdylib the binding loads via koffi is built first in the rust
# image and carried forward via the shared target/ mount.
[private]
_typescript_container +ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "${DEVCONTAINER:-}" = "true" ]; then
        just --justfile "{{TOP}}/justfile.container" {{ARGS}}
    else
        {{CONTAINER_RUN}} --network=host \
            -v "{{TOP}}:/workspace:Z" \
            -v "{{TOP}}/justfile.container:/workspace/justfile:ro" \
            -w /workspace \
            -e ANGZARR_PROJECT_PROTO=/workspace/angzarr-project/proto \
            "{{ROUTER_TYPESCRIPT_IMAGE}}" just {{ARGS}}
    fi

# Build the workspace
build: (_container "build")

# Run the unit test bank
test: (_container "test")

# Mutation-test the core modules
mutation-test: (_container "mutation-test")

# Format
fmt: (_container "fmt")

# Lint (clippy, warnings are errors)
lint: (_container "lint")

# Lint the router's protos (buf, pinned plugins)
buf-lint: (_container "buf-lint")

# Check proto formatting
buf-format: (_container "buf-format")

# --- Go binding (bindings/go) -------------------------------------------
# Runs in the Go image (like client-go); the router-ffi cdylib is built in
# the rust image (`build`) and carried forward via the shared target/ mount
# — the cdylib is the ABI boundary, so passing it between images mirrors the
# architecture. No unified all-languages image needed.

# Regenerate the Go binding's protobuf types (buf + protoc-gen-go)
go-binding-gen: (_go_container "go-binding-gen")

# Build the Go binding (cdylib in the rust image, then go in the go image)
go-binding-build: build (_go_container "go-binding-build")

# Run the Go binding's conformance suite (godog) + property sweep
go-binding-test: build (_go_container "go-binding-test")

# Format check + vet the Go binding
go-binding-lint: (_go_container "go-binding-lint")

# --- Python binding (bindings/python) -----------------------------------
# Runs in the Python image (like client-python); the router-ffi cdylib is
# built in the rust image (`build`) and carried forward via the shared
# target/ mount — dlopen'd by cffi. The ABI is exercised a second way (cffi
# vs cgo) before it freezes (§4). Generated protobuf code is never committed
# (regenerate on need), so build/test regenerate first.

# Regenerate the Python binding's protobuf types (buf + import fixup)
py-binding-gen: (_py_container "py-binding-gen")

# Build the Python binding env (cdylib in the rust image, then uv sync)
py-binding-build: build (_py_container "py-binding-build")

# Run the Python binding's conformance suite (pytest-bdd) + property sweep
py-binding-test: build (_py_container "py-binding-test")

# Lint + format check the Python binding (ruff)
py-binding-lint: (_py_container "py-binding-lint")

# --- Java binding (bindings/java) ----------------------------------------
# Runs in the Java image; the router-ffi cdylib is built in the rust image
# (`build`) and carried forward via the shared target/ mount — loaded in-process
# via Panama/FFM (no preview flags on JDK 25). Generated protobuf + angzarr
# wiring is never committed (regenerate on need), so build/test regenerate first.

# Regenerate the Java binding's protobuf types + angzarr dispatch wiring (buf)
java-binding-gen: (_java_container "java-binding-gen")

# Build the Java binding (cdylib in the rust image, then gradle in the java image)
java-binding-build: build (_java_container "java-binding-build")

# Run the Java binding's conformance suite (Cucumber-JVM) + property sweep
java-binding-test: build (_java_container "java-binding-test")

# Lint + format check the Java binding
java-binding-lint: (_java_container "java-binding-lint")

# --- C# binding (bindings/csharp) ----------------------------------------
# Runs in the C# image; the router-ffi cdylib is built in the rust image
# (`build`) and carried forward via the shared target/ mount — loaded in-process
# via P/Invoke. Generated protobuf + angzarr wiring is never committed.

# Regenerate the C# binding's protobuf types + angzarr wiring (buf) + features
csharp-binding-gen: (_csharp_container "csharp-binding-gen")

# Build the C# binding (cdylib in the rust image, then dotnet in the csharp image)
csharp-binding-build: build (_csharp_container "csharp-binding-build")

# Run the C# binding's conformance suite (Reqnroll/NUnit)
csharp-binding-test: build (_csharp_container "csharp-binding-test")

# Lint + format check the C# binding (csharpier)
csharp-binding-lint: (_csharp_container "csharp-binding-lint")

# Auto-format the C# binding (csharpier)
csharp-binding-format: (_csharp_container "csharp-binding-format")

# --- C++ binding (bindings/cpp) ------------------------------------------
# Runs in the C++ image; the router-ffi STATICLIB is built in the rust image
# (`build`) and carried forward via the shared target/ mount — linked directly
# (no runtime .so). Generated protobuf + angzarr wiring is never committed.

# Regenerate the C++ binding's protobuf types + angzarr wiring (buf)
cpp-binding-gen: (_cpp_container "cpp-binding-gen")

# Build the C++ binding (staticlib in the rust image, then cmake in the cpp image)
cpp-binding-build: build (_cpp_container "cpp-binding-build")

# Run the C++ binding's conformance suite (Catch2 feature-runner)
cpp-binding-test: build (_cpp_container "cpp-binding-test")

# Format check the C++ binding (clang-format)
cpp-binding-lint: (_cpp_container "cpp-binding-lint")
