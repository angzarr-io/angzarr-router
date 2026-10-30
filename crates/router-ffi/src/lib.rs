//! angzarr-router-ffi — the router's generic C ABI (owner's ruling,
//! 2026-10-29). The ABI is data: proto/io/angzarr/router/ffi/v1/abi.proto
//! defines the one component a router hosts (ComponentSpec: a kind and its
//! roles), the one call that drives it (DispatchRequest/DispatchResponse)
//! and each role's Call and Return; tools/abigen generates the bindings
//! from it. Dispatch semantics live in the core; this layer owns the
//! marshaling.
//!
//! # Entry points
//! Seven, and no per-kind symbol: `angzarr_abi_version`, `angzarr_buf_alloc`,
//! `angzarr_buf_release`, `angzarr_router_new`, `angzarr_router_free`,
//! `angzarr_router_register` and `angzarr_router_dispatch`
//! (angzarr_abigen::SYMBOLS declares them for the bindings).
//!
//! # Status codes
//! Every entry point and host callback returns `i32`:
//! - `0`  — success; `out` carries the payload (possibly empty bytes)
//! - `1`  — success with NO payload (the Return message's default)
//! - `<0` — coded failure; the value is the negated gRPC status code and
//!   `out` carries serialized `google.rpc.Status` bytes: message = the key,
//!   a `google.rpc.ErrorInfo` (reason = the key, domain `angzarr.io`,
//!   metadata = the args) and, for a typed refusal, the refusal object
//!   (decision 14.37: no English crosses)
//!
//! # Memory ownership (symmetric, copy-at-the-boundary)
//! - Router → host: `type_url`/`payload`/`aux` buffers are router-owned
//!   and valid ONLY for the duration of the callback.
//! - Host → router: a callback fills `out.data` with memory obtained from
//!   `angzarr_buf_alloc`; the router consumes and frees it. Bindings
//!   never free router memory with their own allocator.
//! - Router → host responses: `angzarr_router_register` and
//!   `angzarr_router_dispatch` fill `out` with router-allocated bytes; the
//!   host copies what it needs and releases them with `angzarr_buf_release`.
//!
//! # Threading and panics
//! Callbacks are invoked synchronously on the dispatching thread, one
//! callback at a time per dispatch; dispatches on different threads may run
//! concurrently against one router. Registration takes the router
//! exclusively: it must not run concurrently with any other call on the
//! same router. Every entry point wraps `catch_unwind`; a Rust panic
//! surfaces as UNHANDLED_HANDLER_ERROR with cause HANDLER_PANICKED — never
//! an abort, never an unwind across the boundary.

mod abi;
#[cfg(any(test, feature = "host"))]
pub mod host;
mod proto;
mod registry;

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub use abi::{AngzarrBuf, AngzarrCb, ABI_VERSION, STATUS_OK, STATUS_OK_EMPTY};
/// The ABI's generated messages (abi.proto) and the vendored google.rpc
/// error shapes, for Rust hosts.
pub use proto::{google, io::angzarr::router::ffi::v1 as abi_pb};

use abi::{coded_to_status_bytes, fill_out};
use angzarr_router::error::{codes, CodedError, GrpcCode};
use registry::FfiRouter;

/// The ABI version bindings must check at load: abi.proto's hash.
#[no_mangle]
pub extern "C" fn angzarr_abi_version() -> u32 {
    ABI_VERSION
}

/// Router-provided allocator. Hosts fill callback `out.data` with memory
/// obtained here, so the router frees what it allocated.
#[no_mangle]
pub extern "C" fn angzarr_buf_alloc(len: usize) -> *mut u8 {
    abi::alloc_bytes(len)
}

/// Releases a router-allocated response buffer (an `out` the router
/// filled). Never used on callback input buffers.
///
/// # Safety
/// `ptr`/`len` must be exactly what the router handed out, released once.
#[no_mangle]
pub unsafe extern "C" fn angzarr_buf_release(ptr: *mut u8, len: usize) {
    abi::free_bytes(ptr, len)
}

/// A new, empty router.
#[no_mangle]
pub extern "C" fn angzarr_router_new() -> *mut c_void {
    Box::into_raw(Box::new(FfiRouter::new())) as *mut c_void
}

/// Frees a router created by `angzarr_router_new`.
///
/// # Safety
/// `r` must be a pointer returned by `angzarr_router_new`, freed once.
#[no_mangle]
pub unsafe extern "C" fn angzarr_router_free(r: *mut c_void) {
    if !r.is_null() {
        drop(Box::from_raw(r as *mut FfiRouter));
    }
}

/// Registers the router's one component from a serialized
/// `io.angzarr.router.ffi.v1.ComponentSpec` and the host's callback gateway.
/// Returns 0 on success (`out` empty); on failure the negated gRPC code,
/// `out` its `google.rpc.Status`. The host releases a filled `out` with
/// `angzarr_buf_release`.
///
/// # Safety
/// `r` must be a live router or null; `spec` must point to `spec_len`
/// readable bytes; `cb` must remain valid for the router's lifetime; `out`
/// must point to a writable `AngzarrBuf`.
#[no_mangle]
pub unsafe extern "C" fn angzarr_router_register(
    r: *mut c_void,
    spec: *const u8,
    spec_len: usize,
    cb: AngzarrCb,
    out: *mut AngzarrBuf,
) -> i32 {
    entry(out, || {
        if r.is_null() {
            return Err(null_pointer());
        }
        let router = &mut *(r as *mut FfiRouter);
        router.register(slice_from(spec, spec_len), cb)?;
        Ok(Vec::new())
    })
}

/// Runs one serialized `io.angzarr.router.ffi.v1.DispatchRequest` through
/// the hosted component. On success returns 0 and fills `out` with the
/// `DispatchResponse`; on failure returns the negated gRPC code and fills
/// `out` with `google.rpc.Status` bytes. Either way the host releases `out`
/// with `angzarr_buf_release`.
///
/// # Safety
/// `r` must be a live router or null; `request` must point to
/// `request_len` readable bytes; `out` must point to a writable
/// `AngzarrBuf`.
#[no_mangle]
pub unsafe extern "C" fn angzarr_router_dispatch(
    r: *mut c_void,
    request: *const u8,
    request_len: usize,
    out: *mut AngzarrBuf,
) -> i32 {
    entry(out, || {
        if r.is_null() {
            return Err(null_pointer());
        }
        let router = &*(r as *const FfiRouter);
        router.dispatch(slice_from(request, request_len))
    })
}

/// Runs an entry point's body, answering 0 and its bytes, or the negated
/// gRPC code and the error's Status; a panic anywhere, the body's or the
/// encoding's, is answered rather than unwound.
fn entry(out: *mut AngzarrBuf, body: impl FnOnce() -> Result<Vec<u8>, CodedError>) -> i32 {
    let err = match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(bytes)) => {
            fill_out(out, &bytes);
            return 0;
        }
        Ok(Err(err)) => err,
        Err(_) => CodedError::panicked(),
    };
    let code = err.grpc as i32;
    match catch_unwind(AssertUnwindSafe(|| coded_to_status_bytes(&err))) {
        Ok(bytes) => fill_out(out, &bytes),
        Err(_) => fill_out(out, &[]),
    }
    -code
}

unsafe fn slice_from<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

/// A null router pointer crossing the ABI: NULL_ABI_POINTER, a binding bug
/// (INTERNAL).
fn null_pointer() -> CodedError {
    CodedError {
        code: codes::NULL_ABI_POINTER.to_string(),
        extras: Default::default(),
        grpc: GrpcCode::Internal,
        refusal: None,
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod lib_tests;
