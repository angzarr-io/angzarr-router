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
# Pinned org toolchain image (rust + buf + protoc + prost/tonic plugins).
# Override with ANGZARR_ROUTER_IMAGE to pin a specific git-SHA tag.
ROUTER_IMAGE := env_var_or_default("ANGZARR_ROUTER_IMAGE", "ghcr.io/angzarr-io/angzarr-rust:latest")
# Container runtime: docker (rootless or rootful). Empty inside a container.
CONTAINER_CMD := `command -v docker 2>/dev/null || echo ""`
# `-u $(id -u):$(id -g)` is right for ROOTFUL docker (bind-mount files get the
# host UID). With ROOTLESS docker it is WRONG: the userns already maps
# container-root → host UID, so -u remaps onto an unowned subuid and breaks
# bind-mount writes. Detect rootless via the daemon and skip -u.
CONTAINER_USER_ARG := if `docker info 2>/dev/null | grep -q rootless && echo yes || echo no` == "yes" { "" } else { "-u $(id -u):$(id -g)" }
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
