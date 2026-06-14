//! angzarr-router-ffi — the C ABI over the core. Nothing else: no
//! semantics in this layer.
//!
//! # Status codes
//! Every entry point and host callback returns `i32`:
//! - `0`  — success; `out` carries the payload (possibly empty bytes)
//! - `1`  — success with NO payload (a handler that emits nothing)
//! - `<0` — coded failure; the value is the negated gRPC status code and
//!   `out` carries serialized `google.rpc.Status` bytes with a
//!   `google.rpc.ErrorInfo` detail (reason = SCREAMING_SNAKE code,
//!   domain `angzarr.io`, metadata = extras)
//!
//! # Memory ownership (symmetric, copy-at-the-boundary)
//! - Router → host: `type_url`/`payload`/`aux` buffers are router-owned
//!   and valid ONLY for the duration of the callback.
//! - Host → router: a callback fills `out.data` with memory obtained from
//!   `angzarr_buf_alloc`; the router consumes and frees it. Bindings
//!   never free router memory with their own allocator.
//! - Router → host responses: `angzarr_router_dispatch` fills `out` with
//!   router-allocated bytes; the host copies what it needs and releases
//!   them with `angzarr_buf_release`.
//!
//! # Threading and panics
//! Callbacks are invoked synchronously on the dispatching thread — one
//! callback at a time per dispatch; dispatches on different host_ctx
//! values may run concurrently. `host_ctx` is opaque to Rust: it is where
//! the binding parks the per-dispatch state object (state never crosses).
//! Every entry point wraps `catch_unwind`; a Rust panic surfaces as a
//! coded UNHANDLED_HANDLER_ERROR failure — never an abort, never an
//! unwind across the boundary.

mod abi;
mod proto;
mod registry;

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub use abi::{AngzarrBuf, AngzarrCb};

use abi::{coded_to_status_bytes, fill_out, ABI_VERSION};
use angzarr_router::error::CodedError;
use registry::FfiRouter;

/// The ABI version bindings must check at load.
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

/// Releases a router-allocated response buffer (the `out` filled by
/// `angzarr_router_dispatch`). Never used on callback input buffers.
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

/// Registers one aggregate component from a serialized
/// `angzarr.router.ffi.v1.AggregateDescriptor` and the host's callback
/// gateway. Returns 0 on success, a negated gRPC code on failure.
///
/// # Safety
/// `r` must be a live router; `descriptor` must point to `descriptor_len`
/// readable bytes; `cb` must remain valid for the router's lifetime.
#[no_mangle]
pub unsafe extern "C" fn angzarr_router_register_aggregate(
    r: *mut c_void,
    descriptor: *const u8,
    descriptor_len: usize,
    cb: AngzarrCb,
) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if r.is_null() {
            return Err(invalid_pointer("router"));
        }
        let router = &mut *(r as *mut FfiRouter);
        let bytes = slice_from(descriptor, descriptor_len);
        router.register_aggregate(bytes, cb)
    }));
    match flatten_panic(result) {
        Ok(()) => 0,
        Err(err) => -(err.grpc as i32),
    }
}

/// Dispatches `ContextualCommand` bytes through the registered tables.
/// On success returns 0 and fills `out` with `BusinessResponse` bytes; on
/// failure returns the negated gRPC code and fills `out` with
/// `google.rpc.Status` bytes. Either way the host releases `out` with
/// `angzarr_buf_release`.
///
/// # Safety
/// `r` must be a live router; `request` must point to `request_len`
/// readable bytes; `out` must point to a writable `AngzarrBuf`.
#[no_mangle]
pub unsafe extern "C" fn angzarr_router_dispatch(
    r: *mut c_void,
    host_ctx: *mut c_void,
    request: *const u8,
    request_len: usize,
    out: *mut AngzarrBuf,
) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if r.is_null() {
            return Err(invalid_pointer("router"));
        }
        let router = &*(r as *const FfiRouter);
        let bytes = slice_from(request, request_len);
        router.dispatch(host_ctx, bytes)
    }));
    match flatten_panic(result) {
        Ok(response) => {
            fill_out(out, &response);
            0
        }
        Err(err) => {
            let code = err.grpc as i32;
            fill_out(out, &coded_to_status_bytes(err));
            -code
        }
    }
}

unsafe fn slice_from<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

fn invalid_pointer(what: &str) -> CodedError {
    CodedError::unhandled(&format!("null {what} pointer crossed the ABI"))
}

/// A panic anywhere inside an entry point becomes a coded
/// UNHANDLED_HANDLER_ERROR — never an unwind across the boundary.
fn flatten_panic<T>(
    result: std::thread::Result<Result<T, CodedError>>,
) -> Result<T, CodedError> {
    match result {
        Ok(inner) => inner,
        // &*panic: pass the payload itself, not the Box (which is also Any).
        Err(panic) => Err(CodedError::unhandled(&panic_message(&*panic))),
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic crossed the FFI entry point".to_string()
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod lib_tests;
