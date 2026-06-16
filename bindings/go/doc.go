// Package ffirouter is the Go binding over the angzarr-router Rust core's
// C ABI (crates/router-ffi). It exposes an engine-shaped registration API
// and dispatch, marshaling commands and responses across the FFI boundary;
// host state never crosses — the per-dispatch session lives only on the Go
// side, reached from callbacks via the host_ctx handle.
//
// The cgo surface is gated behind the `ffirouter` build tag so a plain
// `go build ./...` stays pure-Go until the artifact/packaging story lands.
// Build and test with `-tags ffirouter` and a libangzarr_router_ffi the
// linker can find (the justfile recipes set this; ANGZARR_ROUTER_LIB /
// CGO_LDFLAGS override the in-repo default).
package ffirouter
