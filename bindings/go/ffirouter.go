//go:build ffirouter

package ffirouter

/*
// Cross-platform link to the router-ffi shared library (cdylib): cargo
// emits libangzarr_router_ffi.so on Linux, .dylib on macOS, and
// angzarr_router_ffi.dll (+ import lib) on Windows from
// crate-type=cdylib — the right one for whatever host target built it.
//
// Dynamic linking is deliberate: it keeps the link flags identical across
// the three OSes because the shared lib already resolves its own native
// dependencies. Static linking the .a would instead force a per-OS list of
// Rust std's native libs (on Linux: -lgcc_s -lutil -lrt -lpthread -lm -ldl
// -lc, from `cargo rustc -- --print native-static-libs`; macOS and Windows
// differ), which is the cross-platform footgun we avoid here. Self-
// contained static binaries are a packaging-time concern (deferred, §7).
//
// The rpath points the loader at the in-repo build dir so `go test` finds
// the lib without LD_LIBRARY_PATH/DYLD_LIBRARY_PATH. Windows has no rpath:
// the .dll must sit on PATH or beside the binary (the justfile recipe
// arranges this). The in-repo default is target/debug; override the
// search+rpath dir via CGO_LDFLAGS (justfile / ANGZARR_ROUTER_LIB).
#cgo linux LDFLAGS: -L${SRCDIR}/../../target/debug -langzarr_router_ffi -Wl,-rpath,${SRCDIR}/../../target/debug
#cgo darwin LDFLAGS: -L${SRCDIR}/../../target/debug -langzarr_router_ffi -Wl,-rpath,${SRCDIR}/../../target/debug
#cgo windows LDFLAGS: -L${SRCDIR}/../../target/debug -langzarr_router_ffi
#include <stdint.h>

uint32_t angzarr_abi_version(void);
*/
import "C"

// AbiVersion reports the ABI version the linked router-ffi exposes.
// Bindings check it at load so a binding and a router-ffi artifact that
// have drifted refuse each other instead of marshaling garbage.
func AbiVersion() uint32 {
	return uint32(C.angzarr_abi_version())
}
