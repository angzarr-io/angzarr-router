//! The raw ABI vocabulary: buffer type, allocator, status-code
//! conventions, the ABI version, and the error serialization both
//! directions share.

use angzarr_router::error::{CodedError, GrpcCode};

/// The ABI version: abi.proto's FNV-1a 32-bit hash (abi.proto's header;
/// angzarr-abigen computes the same for the bindings it generates). Any
/// edit of abi.proto is a new ABI.
pub const ABI_VERSION: u32 = fnv1a32(include_bytes!(
    "../../../proto/io/angzarr/router/ffi/v1/abi.proto"
));

const fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

/// Callback success with a payload in `out`.
pub const STATUS_OK: i32 = 0;
/// Callback success with NO payload (the role's Return message's default).
pub const STATUS_OK_EMPTY: i32 = 1;

/// The byte-buffer shape crossing the boundary.
#[repr(C)]
#[derive(Debug)]
pub struct AngzarrBuf {
    pub data: *mut u8,
    pub len: usize,
}

/// The single host callback signature; `callback_id` selects the host
/// thunk (a role's RoleEntry.callback_id), `type_url` names the role's Call
/// message and `payload` carries it (abi.proto, the callback convention).
pub type AngzarrCb = unsafe extern "C" fn(
    callback_id: u64,
    type_url: *const u8,
    type_url_len: usize,
    payload: *const u8,
    payload_len: usize,
    aux: *const u8,
    aux_len: usize,
    out: *mut AngzarrBuf,
) -> i32;

/// Allocates `len` zeroed bytes from the router's allocator as an
/// exact-size boxed slice (its allocation size is `len`, by construction).
/// Returns null for `len == 0`.
pub fn alloc_bytes(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    Box::into_raw(vec![0u8; len].into_boxed_slice()).cast::<u8>()
}

/// Reclaims an `alloc_bytes(len)` allocation as its boxed slice.
///
/// # Safety
/// `ptr`/`len` must be exactly a non-null `alloc_bytes(len)` result with
/// `len > 0`, reclaimed once.
unsafe fn reclaim(ptr: *mut u8, len: usize) -> Box<[u8]> {
    Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len))
}

/// Frees bytes produced by `alloc_bytes` / handed out via `fill_out`.
///
/// # Safety
/// `ptr`/`len` must be exactly an `alloc_bytes(len)` result, freed once.
pub unsafe fn free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        drop(reclaim(ptr, len));
    }
}

/// Fills a host-supplied `out` with router-allocated bytes (host releases
/// via `angzarr_buf_release`).
pub fn fill_out(out: *mut AngzarrBuf, bytes: &[u8]) {
    if out.is_null() {
        return;
    }
    let buf = unsafe { &mut *out };
    if bytes.is_empty() {
        buf.data = std::ptr::null_mut();
        buf.len = 0;
        return;
    }
    let ptr = alloc_bytes(bytes.len());
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
    buf.data = ptr;
    buf.len = bytes.len();
}

/// Takes ownership of a host-filled `out` buffer (memory from
/// `angzarr_buf_alloc`), returning its bytes and freeing the allocation.
// The registry's, stubbed until the green commit.
#[cfg_attr(not(test), allow(dead_code))]
pub fn consume_out(out: &mut AngzarrBuf) -> Option<Vec<u8>> {
    if out.data.is_null() || out.len == 0 {
        return None;
    }
    // The host contract: `data` came from angzarr_buf_alloc(len) with `len`
    // the requested size.
    let bytes = unsafe { reclaim(out.data, out.len) }.into_vec();
    out.data = std::ptr::null_mut();
    out.len = 0;
    Some(bytes)
}

/// Serializes a CodedError as google.rpc.Status bytes, the exact shape gRPC
/// puts on the wire (decision 14.37): code = its gRPC status, message = its
/// key, an ErrorInfo detail (reason = the key, domain angzarr.io, metadata
/// = its args) and, for a typed business refusal, the refusal object as a
/// second detail.
pub fn coded_to_status_bytes(err: &CodedError) -> Vec<u8> {
    let _ = err;
    todo!("coded_to_status_bytes")
}

/// Decodes a host callback's failure: `ret` its (negative) return and
/// `bytes` its `out`. A google.rpc.Status crosses exactly: its code, the
/// ErrorInfo's reason and metadata, and a typed refusal object. Bytes that
/// do not decode are HOST_STATUS_UNDECODABLE (INTERNAL), prost's text as
/// `ext_err_txt`; no bytes at all, UNHANDLED_HANDLER_ERROR under the status
/// `-ret` names.
// The registry's, stubbed until the green commit.
#[cfg_attr(not(test), allow(dead_code))]
pub fn status_to_coded(bytes: Option<&[u8]>, ret: i32) -> CodedError {
    let _ = (bytes, ret);
    todo!("status_to_coded")
}

/// Wire code → GrpcCode: every code the router knows crosses as itself;
/// any other (OK, CANCELLED, UNAUTHENTICATED, an unassigned number)
/// degrades to INTERNAL (a binding emitting one is a binding bug).
// The registry's, stubbed until the green commit.
#[cfg_attr(not(test), allow(dead_code))]
pub fn grpc_from_wire(code: i32) -> GrpcCode {
    let _ = code;
    todo!("grpc_from_wire")
}

#[cfg(test)]
#[path = "abi.test.rs"]
mod abi_tests;
