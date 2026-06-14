//! The raw ABI vocabulary: buffer type, allocator, status-code
//! conventions, and the error serialization both directions share.

use prost::Message;

use angzarr_router::error::{CodedError, GrpcCode, ERROR_INFO_DOMAIN};

use crate::proto::google::rpc::{ErrorInfo, Status};

pub const ABI_VERSION: u32 = 1;

/// Callback success with a payload in `out`.
pub const STATUS_OK: i32 = 0;
/// Callback success with NO payload (handler emitted nothing).
pub const STATUS_OK_EMPTY: i32 = 1;

/// The byte-buffer shape crossing the boundary.
#[repr(C)]
#[derive(Debug)]
pub struct AngzarrBuf {
    pub data: *mut u8,
    pub len: usize,
}

/// The single host callback signature; `callback_id` selects the host
/// thunk (the binding's registration API assigns ids).
pub type AngzarrCb = unsafe extern "C" fn(
    host_ctx: *mut std::ffi::c_void,
    callback_id: u64,
    type_url: *const u8,
    type_url_len: usize,
    payload: *const u8,
    payload_len: usize,
    aux: *const u8,
    aux_len: usize,
    out: *mut AngzarrBuf,
) -> i32;

/// Allocates `len` zeroed bytes from the router's allocator. Returns null
/// for `len == 0`.
pub fn alloc_bytes(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    let mut v = vec![0u8; len];
    let ptr = v.as_mut_ptr();
    std::mem::forget(v);
    ptr
}

/// Frees bytes produced by `alloc_bytes` / handed out via `fill_out`.
///
/// # Safety
/// `ptr`/`len` must be exactly an `alloc_bytes(len)` result, freed once.
pub unsafe fn free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        drop(Vec::from_raw_parts(ptr, len, len));
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
pub fn consume_out(out: &mut AngzarrBuf) -> Option<Vec<u8>> {
    if out.data.is_null() || out.len == 0 {
        return None;
    }
    let bytes = unsafe { Vec::from_raw_parts(out.data, out.len, out.len) };
    out.data = std::ptr::null_mut();
    out.len = 0;
    Some(bytes)
}

const ERROR_INFO_TYPE_URL: &str = "type.googleapis.com/google.rpc.ErrorInfo";

/// Serializes a CodedError as google.rpc.Status bytes carrying an
/// ErrorInfo detail — the exact shape gRPC puts on the wire.
pub fn coded_to_status_bytes(err: CodedError) -> Vec<u8> {
    let info = ErrorInfo {
        reason: err.code,
        domain: ERROR_INFO_DOMAIN.to_string(),
        metadata: err.extras.into_iter().collect(),
    };
    Status {
        code: err.grpc as i32,
        message: err.message,
        details: vec![prost_types::Any {
            type_url: ERROR_INFO_TYPE_URL.to_string(),
            value: info.encode_to_vec(),
        }],
    }
    .encode_to_vec()
}

/// Decodes host-returned google.rpc.Status bytes into a CodedError.
/// `ret` (the negative callback return) is the fallback gRPC code when
/// the Status bytes are absent or carry no usable code.
pub fn status_to_coded(bytes: Option<&[u8]>, ret: i32) -> CodedError {
    let fallback = grpc_from_wire(-ret);
    let Some(bytes) = bytes else {
        return CodedError {
            code: String::new(),
            message: "host callback failed without a status payload".to_string(),
            extras: Default::default(),
            grpc: fallback,
        };
    };
    let Ok(status) = Status::decode(bytes) else {
        return CodedError::unhandled("host callback returned undecodable status bytes");
    };
    let mut code = String::new();
    let mut extras = std::collections::BTreeMap::new();
    for detail in &status.details {
        if detail.type_url == ERROR_INFO_TYPE_URL {
            if let Ok(info) = ErrorInfo::decode(detail.value.as_slice()) {
                code = info.reason;
                extras = info.metadata.into_iter().collect();
            }
            break;
        }
    }
    CodedError {
        code,
        message: status.message,
        extras,
        grpc: if status.code != 0 {
            grpc_from_wire(status.code)
        } else {
            fallback
        },
    }
}

/// Wire code → GrpcCode; unknown codes degrade to INTERNAL (a binding
/// emitting an unmapped code is a binding bug).
pub fn grpc_from_wire(code: i32) -> GrpcCode {
    match code {
        3 => GrpcCode::InvalidArgument,
        5 => GrpcCode::NotFound,
        9 => GrpcCode::FailedPrecondition,
        12 => GrpcCode::Unimplemented,
        15 => GrpcCode::DataLoss,
        _ => GrpcCode::Internal,
    }
}

#[cfg(test)]
#[path = "abi.test.rs"]
mod abi_tests;
