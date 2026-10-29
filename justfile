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
# Each is pinned by tag and digest (the image actually tested); override with
# the matching env var.
ROUTER_IMAGE := env_var_or_default("ANGZARR_ROUTER_IMAGE", "ghcr.io/angzarr-io/angzarr-rust:9e07ae0@sha256:1f70b5de243d50aab989103ce87be393b6282c538ecabccef5f07c15760ac156")
ROUTER_GO_IMAGE := env_var_or_default("ANGZARR_ROUTER_GO_IMAGE", "ghcr.io/angzarr-io/angzarr-go:latest@sha256:b57cce65c7bc14aaa67845d94d0eb3c7f7bddae43190299ec8f02bb8e92ee6d8")
ROUTER_JAVA_IMAGE := env_var_or_default("ANGZARR_ROUTER_JAVA_IMAGE", "ghcr.io/angzarr-io/angzarr-java:531d91e@sha256:3c64d5337aa53c1a5a2c7bf34737b7012464acfdb5c2dc39f55a119f3441e96e")
ROUTER_CSHARP_IMAGE := env_var_or_default("ANGZARR_ROUTER_CSHARP_IMAGE", "ghcr.io/angzarr-io/angzarr-csharp:latest@sha256:32c482820b2021ec7639a800d8778a51db27e604edbe1359a8a328c6c65b5991")
ROUTER_CPP_IMAGE := env_var_or_default("ANGZARR_ROUTER_CPP_IMAGE", "ghcr.io/angzarr-io/angzarr-cpp:latest@sha256:3c66dd0ffc7d2dd727c355d1b22c2741abce4d570baea4517c77fd5d40d97bfe")
ROUTER_TYPESCRIPT_IMAGE := env_var_or_default("ANGZARR_ROUTER_TYPESCRIPT_IMAGE", "ghcr.io/angzarr-io/angzarr-typescript:531d91e@sha256:6121d654662bcbba25162d89b6fe03d41b6d9c364346dd7e6fee394123ef02b3")
# Container runtime: docker (rootless or rootful). Empty inside a container.
CONTAINER_CMD := `command -v docker 2>/dev/null || echo ""`
# `-u $(id -u):$(id -g)` is right for ROOTFUL docker (bind-mount files get the
# host UID). With ROOTLESS docker that is WRONG: the userns maps
# container-root → host UID, so -u $(id -u) remaps onto an unowned subuid and
# breaks bind-mount writes. Force -u 0:0 instead of relying on the image's
# default user — images that set a non-root USER (the python image runs as
# `angzarr`) otherwise land on a subuid that cannot write the mount (and trips
# git's dubious-ownership guard). Running as container-root maps to the host UID.
# ANGZARR_CONTAINER_USER overrides it (CI runs the toolchains as container-root
# so their HOME-based caches work).
CONTAINER_USER_ARG := if env_var_or_default("ANGZARR_CONTAINER_USER", "") != "" { "-u " + env_var("ANGZARR_CONTAINER_USER") } else if `docker info 2>/dev/null | grep -q rootless && echo yes || echo no` == "yes" { "-u 0:0" } else { "-u $(id -u):$(id -g)" }
# The checkout is owned by the host user, not the container user: let git use it.
CONTAINER_GIT_ENV := "-e GIT_CONFIG_COUNT=1 -e GIT_CONFIG_KEY_0=safe.directory -e GIT_CONFIG_VALUE_0='*'"
CONTAINER_RUN := CONTAINER_CMD + " run --rm " + CONTAINER_USER_ARG + " " + CONTAINER_GIT_ENV

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

# Check Rust formatting (no rewrite; fails on drift)
fmt-check: (_container "fmt-check")

# Build the pinned angzarr CLI (the codegen plugin) into .tools/bin
cli: (_go_container "cli")

# Mutation-test the core modules and the FFI registry: the full pass
mutation-test: (_container "mutation-test")

# Mutation-test what a push adds: lefthook's pre-push step (the full pass,
# `mutation-test`, is too slow for a hook). For each ref pushed (git's pushed
# refs on stdin), the diff from its base to the commit pushed is mutated
# (`mutation-diff`: the MUTATED files only, held to 100%), in that commit's
# tree checked out alone at target/push-tree, so an unpushed edit cannot pass
# or fail it. The base is the merge base of the commit pushed and the remote's
# current tip of that ref (a fast-forward's base is that tip); for a new ref,
# or a tip not fetched here, the merge base with the remote's default branch
# (<remote>/HEAD, else main). A deleted ref, and a ref whose diff touches no
# Rust, pass at once. The push tree's angzarr-project is the pushed commit's
# submodule commit, exported from this checkout's submodule (fetched if it is
# missing there). The run's log is target/logs/mutation-push.log, its
# mutants.out target/push-tree/target/mutants-push/mutants.out.
#
# Red tests (cargo-mutants' baseline fails, exit 4) cannot be judged: pushed
# to a branch other than the default one they pass, with a warning that the
# branch cannot merge until they pass; pushed to the default branch they are
# refused. `lefthook` is what lefthook.yml passes to stop lefthook skipping
# it, and is ignored.
[positional-arguments]
mutation-push remote *lefthook:
    #!/usr/bin/env bash
    set -euo pipefail
    # git exports repository-locating variables (GIT_DIR, ...) into a hook;
    # with them, a git -C the push tree or a submodule would act on this
    # checkout instead.
    unset $(git rev-parse --local-env-vars)
    top='{{TOP}}'
    remote=$1
    default=$(git -C "$top" symbolic-ref --quiet --short "refs/remotes/$remote/HEAD" 2>/dev/null) && default=${default#"$remote"/} || default=main
    tree="$top/target/push-tree"
    mkdir -p "$top/target/logs"
    log="$top/target/logs/mutation-push.log"
    exec 9> "$top/target/.push-tree.lock"
    flock 9
    pushed=0
    while read -r local_ref local_sha remote_ref remote_sha; do
        [ -n "${local_sha:-}" ] || continue
        [ "${local_sha//0/}" = "" ] && continue  # a deletion
        pushed=1
        target=${remote_ref#refs/heads/}
        base=
        if [ -n "${remote_sha//0/}" ] && git -C "$top" cat-file -e "$remote_sha^{commit}" 2>/dev/null; then
            base=$(git -C "$top" merge-base "$remote_sha" "$local_sha") || base=
        fi
        if [ -z "$base" ]; then
            for ref in "refs/remotes/$remote/$default" "refs/heads/$default"; do
                if git -C "$top" rev-parse --verify --quiet "$ref^{commit}" > /dev/null; then
                    base=$(git -C "$top" merge-base "$ref" "$local_sha") && break
                fi
            done
        fi
        if [ -z "$base" ]; then
            echo "FAIL mutation-push: $local_ref shares no history with $remote's tip or with $default" >&2
            exit 1
        fi
        range="$(git -C "$top" rev-parse --short "$base")..$(git -C "$top" rev-parse --short "$local_sha") to $remote/$target"
        if [ -z "$(git -C "$top" diff --name-only "$base" "$local_sha" -- '*.rs')" ]; then
            echo "ok   mutation-push ($range: no mutants, the push touches no Rust)"
            continue
        fi
        # The pushed commit's tree, alone; no hook of this repository runs for
        # it (core.hooksPath), and its target/ and .cargo-container/ (ignored,
        # so kept by clean) carry builds across pushes.
        nohooks=(-c core.hooksPath=/dev/null)
        if ! git -C "$tree" rev-parse --git-dir > /dev/null 2>&1; then
            rm -rf "$tree"
            git -C "$top" worktree prune
            git "${nohooks[@]}" -C "$top" worktree add --quiet --detach "$tree" "$local_sha"
        fi
        git "${nohooks[@]}" -C "$tree" checkout --quiet --detach --force "$local_sha"
        git -C "$tree" clean -fdq
        sub=$(git -C "$top" ls-tree "$local_sha" angzarr-project | awk '{print $3}')
        if ! git -C "$top/angzarr-project" cat-file -e "$sub^{commit}" 2>/dev/null; then
            git -C "$top/angzarr-project" fetch --quiet origin "$sub" || {
                echo "FAIL mutation-push: angzarr-project $sub, the pushed commit's submodule, is not here (git submodule update --init)" >&2
                exit 1
            }
        fi
        rm -rf "$tree/angzarr-project"
        mkdir -p "$tree/angzarr-project" "$tree/target"
        git -C "$top/angzarr-project" archive "$sub" | tar -x -C "$tree/angzarr-project"
        git -C "$top" diff --no-color --no-ext-diff --no-renames --src-prefix=a/ --dst-prefix=b/ \
            "$base" "$local_sha" -- '*.rs' > "$tree/target/mutants-push.patch"
        rc=0
        (cd "$tree" && just --justfile "$tree/justfile" _container mutation-diff target/mutants-push.patch) > "$log" 2>&1 9>&- || rc=$?
        out="$tree/target/mutants-push/mutants.out"
        case "$rc" in
            0) echo "ok   mutation-push ($range): $(grep -E '^[0-9]+ mutants tested' "$log" | tail -1 || echo 'no mutants in the diff')" ;;
            4)
                if [ "$remote_ref" != "refs/heads/$default" ]; then
                    echo "RED  mutation-push: $range not judged, the tests are red; allowed off $default, which $target cannot merge into until they pass"
                else
                    echo "FAIL mutation-push: $range: red tests cannot be pushed to $remote/$target (full log: target/logs/mutation-push.log)" >&2
                    exit 1
                fi ;;
            *)
                echo "FAIL mutation-push: $range: a mutant survived, timed out, or the run failed (cargo-mutants exit $rc; full log: target/logs/mutation-push.log)" >&2
                for kind in missed timeout; do
                    if [ -s "$out/$kind.txt" ]; then
                        echo "$kind:" >&2
                        sed 's/^/  /' "$out/$kind.txt" >&2
                    fi
                done
                [ -s "$out/missed.txt" ] && echo "Add or sharpen the test that would kill each survivor, commit it, and push again." >&2
                exit 1 ;;
        esac
    done
    [ "$pushed" = 1 ] || echo "ok   mutation-push (no commit pushed: nothing to mutate)"

# Format
fmt: (_container "fmt")

# Lint (clippy, warnings are errors)
lint: (_container "lint")

# Architecture lint: module-level layer rules (archlint.toml)
archlint: (_container "archlint")

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
go-binding-gen: cli (_go_container "go-binding-gen")

# Build the Go binding (cdylib in the rust image, then go in the go image)
go-binding-build: build cli (_go_container "go-binding-build")

# Run the Go binding's conformance suite (godog) + property sweep
go-binding-test: build cli (_go_container "go-binding-test")

# Format check + vet the Go binding
go-binding-lint: cli (_go_container "go-binding-lint")

# --- Java binding (bindings/java) ----------------------------------------
# Runs in the Java image; the router-ffi cdylib is built in the rust image
# (`build`) and carried forward via the shared target/ mount — loaded in-process
# via Panama/FFM (no preview flags on JDK 25). Generated protobuf + angzarr
# wiring is never committed (regenerate on need), so build/test regenerate first.

# Regenerate the Java binding's protobuf types + angzarr dispatch wiring (buf)
java-binding-gen: cli (_java_container "java-binding-gen")

# Build the Java binding (cdylib in the rust image, then gradle in the java image)
java-binding-build: build cli (_java_container "java-binding-build")

# Run the Java binding's conformance suite (Cucumber-JVM) + property sweep
java-binding-test: build cli (_java_container "java-binding-test")

# Lint + format check the Java binding
java-binding-lint: cli (_java_container "java-binding-lint")

# Auto-format the Java binding (spotless)
java-binding-format: cli (_java_container "java-binding-format")

# --- C# binding (bindings/csharp) ----------------------------------------
# Runs in the C# image; the router-ffi cdylib is built in the rust image
# (`build`) and carried forward via the shared target/ mount — loaded in-process
# via P/Invoke. Generated protobuf + angzarr wiring is never committed.

# Regenerate the C# binding's protobuf types + angzarr wiring (buf) + features
csharp-binding-gen: cli (_csharp_container "csharp-binding-gen")

# Build the C# binding (cdylib in the rust image, then dotnet in the csharp image)
csharp-binding-build: build cli (_csharp_container "csharp-binding-build")

# Run the C# binding's conformance suite (Reqnroll/NUnit)
csharp-binding-test: build cli (_csharp_container "csharp-binding-test")

# Lint + format check the C# binding (csharpier)
csharp-binding-lint: (_csharp_container "csharp-binding-lint")

# Auto-format the C# binding (csharpier)
csharp-binding-format: (_csharp_container "csharp-binding-format")

# --- C++ binding (bindings/cpp) ------------------------------------------
# Runs in the C++ image; the router-ffi STATICLIB is built in the rust image
# (`build`) and carried forward via the shared target/ mount — linked directly
# (no runtime .so). Generated protobuf + angzarr wiring is never committed.

# Regenerate the C++ binding's protobuf types + angzarr wiring (buf)
cpp-binding-gen: cli (_cpp_container "cpp-binding-gen")

# Build the C++ binding (staticlib in the rust image, then cmake in the cpp image)
cpp-binding-build: build cli (_cpp_container "cpp-binding-build")

# Run the C++ binding's conformance suite (Catch2 feature-runner)
cpp-binding-test: build cli (_cpp_container "cpp-binding-test")

# Format check the C++ binding (clang-format)
cpp-binding-lint: (_cpp_container "cpp-binding-lint")

# --- TypeScript binding (bindings/typescript) ----------------------------
# Runs in the TypeScript image; the router-ffi cdylib is built in the rust image
# (`build`) and carried forward via the shared target/ mount — loaded in-process
# via koffi. Generated protobuf-es types + angzarr wiring is never committed.

# Regenerate the TS binding's protobuf-es types + angzarr wiring (buf)
ts-binding-gen: cli (_typescript_container "ts-binding-gen")

# Build the TS binding (cdylib in the rust image, then npm ci in the ts image)
ts-binding-build: build cli (_typescript_container "ts-binding-build")

# Run the TS binding's conformance suite (cucumber-js)
ts-binding-test: build cli (_typescript_container "ts-binding-test")

# Format check (prettier) and type-check (tsc) the TS binding
ts-binding-lint: cli (_typescript_container "ts-binding-lint")

# Auto-format the TS binding (prettier)
ts-binding-format: (_typescript_container "ts-binding-format")
