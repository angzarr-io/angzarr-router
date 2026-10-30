//! A Rust host of the C ABI: what a binding is, written in Rust, so the
//! router's own tests and the conformance suite (crates/conformance, the
//! `*_ffi` targets) drive the component through the ABI exactly as a
//! foreign binding does: raw pointers, the one callback, role Calls in and
//! Returns out, errors as google.rpc.Status bytes.
//!
//! It is deliberately independent of the router's side: its own status
//! encoding and decoding, its own callback table, no access to the
//! registry. Only the buffer allocator is shared, as the ABI requires.

use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use prost::{Message, Name};

use angzarr_router::error::{CodedError, GrpcCode, ERROR_INFO_DOMAIN};

use crate::abi::{AngzarrBuf, STATUS_OK, STATUS_OK_EMPTY};
use crate::proto::google::rpc::{ErrorInfo, Status};
use crate::proto::io::angzarr::router::ffi::v1 as abi_pb;

const ERROR_INFO_TYPE_URL: &str = "type.googleapis.com/google.rpc.ErrorInfo";

/// A raw host callback: handed the role's Call message name (`type_url`)
/// and bytes, it answers the return code and the `out` bytes.
pub type RawCallback = dyn Fn(&str, &[u8]) -> (i32, Vec<u8>) + Send + Sync;

fn table() -> &'static Mutex<HashMap<u64, Arc<RawCallback>>> {
    static TABLE: OnceLock<Mutex<HashMap<u64, Arc<RawCallback>>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Registers a raw callback; returns its (process-unique) callback id.
pub fn raw_callback(
    callback: impl Fn(&str, &[u8]) -> (i32, Vec<u8>) + Send + Sync + 'static,
) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    table()
        .lock()
        .expect("callback table")
        .insert(id, Arc::new(callback));
    id
}

/// Registers a typed role callback: it is handed its Call `C` decoded and
/// answers its Return `R` (`None`: STATUS_OK_EMPTY) or a coded failure. A
/// Call of another type, or one that does not decode, is answered
/// UNHANDLED_HANDLER_ERROR: a binding's own bug.
pub fn callback<C, R>(
    role: impl Fn(C) -> Result<Option<R>, CodedError> + Send + Sync + 'static,
) -> u64
where
    C: Message + Default + Name,
    R: Message,
{
    raw_callback(move |type_url, payload| {
        if type_url != C::full_name() {
            return failure(&CodedError::unhandled());
        }
        let Ok(call) = C::decode(payload) else {
            return failure(&CodedError::unhandled());
        };
        match role(call) {
            Ok(Some(ret)) => (STATUS_OK, ret.encode_to_vec()),
            Ok(None) => (STATUS_OK_EMPTY, Vec::new()),
            Err(err) => failure(&err),
        }
    })
}

fn failure(err: &CodedError) -> (i32, Vec<u8>) {
    (-(err.grpc as i32), status_bytes(err))
}

/// The one host callback every role is registered with.
///
/// # Safety
/// Called by the router only, with buffers valid for the call.
pub unsafe extern "C" fn trampoline(
    callback_id: u64,
    type_url: *const u8,
    type_url_len: usize,
    payload: *const u8,
    payload_len: usize,
    _aux: *const u8,
    _aux_len: usize,
    out: *mut AngzarrBuf,
) -> i32 {
    let found = table()
        .lock()
        .ok()
        .and_then(|t| t.get(&callback_id).cloned());
    let Some(callback) = found else {
        return -(GrpcCode::Internal as i32);
    };
    let type_url = std::str::from_utf8(bytes(type_url, type_url_len)).unwrap_or_default();
    let payload = bytes(payload, payload_len);
    let (ret, answer) = catch_unwind(AssertUnwindSafe(|| callback(type_url, payload)))
        .unwrap_or_else(|_| failure(&CodedError::panicked()));
    if !out.is_null() && !answer.is_empty() {
        let ptr = crate::angzarr_buf_alloc(answer.len());
        std::ptr::copy_nonoverlapping(answer.as_ptr(), ptr, answer.len());
        (*out).data = ptr;
        (*out).len = answer.len();
    }
    ret
}

unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

/// The host's encoding of a coded failure (abi.proto, the callback
/// convention): message = the key, ErrorInfo, the refusal object second.
pub fn status_bytes(err: &CodedError) -> Vec<u8> {
    let info = ErrorInfo {
        reason: err.code.clone(),
        domain: ERROR_INFO_DOMAIN.to_string(),
        metadata: err.extras.clone().into_iter().collect(),
    };
    let mut details = vec![prost_types::Any {
        type_url: ERROR_INFO_TYPE_URL.to_string(),
        value: info.encode_to_vec(),
    }];
    details.extend(err.refusal.as_deref().cloned());
    Status {
        code: err.grpc as i32,
        message: err.code.clone(),
        details,
    }
    .encode_to_vec()
}

/// The host's reading of the router's failure (`ret` negative, `bytes` its
/// `out`): the Status as a CodedError. No bytes, or bytes that do not
/// decode, read as an empty key under `-ret`'s status, so a test names what
/// is missing.
pub fn coded_from_status(ret: i32, bytes: &[u8]) -> CodedError {
    let grpc = wire_code(-ret);
    if bytes.is_empty() {
        return blank(grpc);
    }
    let Ok(status) = Status::decode(bytes) else {
        return blank(grpc);
    };
    let mut err = blank(wire_code(status.code));
    for (index, detail) in status.details.into_iter().enumerate() {
        if index == 0 && detail.type_url == ERROR_INFO_TYPE_URL {
            if let Ok(info) = ErrorInfo::decode(detail.value.as_slice()) {
                err.code = info.reason;
                err.extras = info.metadata.into_iter().collect::<BTreeMap<_, _>>();
            }
        } else {
            err.refusal = Some(Box::new(detail));
        }
    }
    err
}

fn blank(grpc: GrpcCode) -> CodedError {
    CodedError {
        code: String::new(),
        extras: BTreeMap::new(),
        grpc,
        refusal: None,
    }
}

/// The wire code as the router names it; anything else INTERNAL.
pub fn wire_code(code: i32) -> GrpcCode {
    use GrpcCode::*;
    [
        Unknown,
        InvalidArgument,
        DeadlineExceeded,
        NotFound,
        AlreadyExists,
        PermissionDenied,
        ResourceExhausted,
        FailedPrecondition,
        Aborted,
        OutOfRange,
        Unimplemented,
        Internal,
        Unavailable,
        DataLoss,
    ]
    .into_iter()
    .find(|known| *known as i32 == code)
    .unwrap_or(Internal)
}

/// A router behind the C ABI, driven as a binding drives it.
pub struct Host {
    router: *mut c_void,
}

impl Default for Host {
    fn default() -> Self {
        Host::new()
    }
}

impl Host {
    /// A new, empty router.
    pub fn new() -> Self {
        Host {
            router: crate::angzarr_router_new(),
        }
    }

    /// `angzarr_router_register` over a spec's bytes: the return code and
    /// the `out` bytes.
    pub fn register_raw(&mut self, spec: &[u8]) -> (i32, Vec<u8>) {
        let mut out = empty_buf();
        let ret = unsafe {
            crate::angzarr_router_register(
                self.router,
                spec.as_ptr(),
                spec.len(),
                trampoline,
                &mut out,
            )
        };
        (ret, take(&mut out))
    }

    /// Registers the router's component.
    pub fn register(&mut self, spec: &abi_pb::ComponentSpec) -> Result<(), CodedError> {
        match self.register_raw(&spec.encode_to_vec()) {
            (0, _) => Ok(()),
            (ret, bytes) => Err(coded_from_status(ret, &bytes)),
        }
    }

    /// `angzarr_router_dispatch` over a request's bytes: the return code and
    /// the `out` bytes.
    pub fn dispatch_raw(&self, request: &[u8]) -> (i32, Vec<u8>) {
        let mut out = empty_buf();
        let ret = unsafe {
            crate::angzarr_router_dispatch(self.router, request.as_ptr(), request.len(), &mut out)
        };
        (ret, take(&mut out))
    }

    /// Runs one request through the hosted component.
    pub fn dispatch(
        &self,
        request: abi_pb::dispatch_request::Request,
    ) -> Result<abi_pb::dispatch_response::Response, CodedError> {
        let request = abi_pb::DispatchRequest {
            request: Some(request),
        };
        match self.dispatch_raw(&request.encode_to_vec()) {
            (0, bytes) => abi_pb::DispatchResponse::decode(bytes.as_slice())
                .ok()
                .and_then(|r| r.response)
                .ok_or_else(|| blank(GrpcCode::Internal)),
            (ret, bytes) => Err(coded_from_status(ret, &bytes)),
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe { crate::angzarr_router_free(self.router) };
    }
}

fn empty_buf() -> AngzarrBuf {
    AngzarrBuf {
        data: std::ptr::null_mut(),
        len: 0,
    }
}

fn take(out: &mut AngzarrBuf) -> Vec<u8> {
    if out.data.is_null() || out.len == 0 {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.data, out.len) }.to_vec();
    unsafe { crate::angzarr_buf_release(out.data, out.len) };
    out.data = std::ptr::null_mut();
    out.len = 0;
    bytes
}
