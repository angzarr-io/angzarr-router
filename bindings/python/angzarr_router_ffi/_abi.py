"""cffi ABI-mode binding to the angzarr-router C ABI (crates/router-ffi).

ABI mode (``ffi.dlopen``) means pure-Python consumption of the same C ABI
the Go binding links — no Rust toolchain, no compile step. The shared
library is the cargo-built cdylib (``.so``/``.dylib``/``.dll`` per host);
``ffi.callback`` handles GIL acquisition when the router calls back from a
dispatch thread.
"""

import os

import cffi

ffi = cffi.FFI()

# The slice's C ABI, restricted to what the dispatch path uses. Mirrors
# crates/router-ffi/src/abi.rs; cffi recognizes the C99 stdint types.
ffi.cdef(
    """
    typedef struct { uint8_t* data; size_t len; } angzarr_buf;

    typedef int32_t (*angzarr_cb)(void* host_ctx, uint64_t callback_id,
        const uint8_t* type_url, size_t type_url_len,
        const uint8_t* payload,  size_t payload_len,
        const uint8_t* aux,      size_t aux_len,
        angzarr_buf* out);

    uint32_t angzarr_abi_version(void);
    uint8_t* angzarr_buf_alloc(size_t len);
    void     angzarr_buf_release(uint8_t* ptr, size_t len);
    void*    angzarr_router_new(void);
    void     angzarr_router_free(void* r);
    int32_t  angzarr_router_register_aggregate(void* r,
                 const uint8_t* descriptor, size_t descriptor_len,
                 angzarr_cb cb);
    int32_t  angzarr_router_dispatch(void* r, void* host_ctx,
                 const uint8_t* request, size_t request_len,
                 angzarr_buf* out);
    int32_t  angzarr_router_register_projector(void* r,
                 const uint8_t* descriptor, size_t descriptor_len,
                 angzarr_cb cb);
    int32_t  angzarr_router_dispatch_projector(void* r, void* host_ctx,
                 const uint8_t* request, size_t request_len,
                 angzarr_buf* out);
    int32_t  angzarr_router_register_saga(void* r,
                 const uint8_t* descriptor, size_t descriptor_len,
                 angzarr_cb cb);
    int32_t  angzarr_router_dispatch_saga(void* r, void* host_ctx,
                 const uint8_t* request, size_t request_len,
                 angzarr_buf* out);
    int32_t  angzarr_router_register_process_manager(void* r,
                 const uint8_t* descriptor, size_t descriptor_len,
                 angzarr_cb cb);
    int32_t  angzarr_router_dispatch_process_manager(void* r, void* host_ctx,
                 const uint8_t* request, size_t request_len,
                 angzarr_buf* out);
    """
)


def _library_path() -> str:
    """Locate the router-ffi shared library.

    ``ANGZARR_ROUTER_LIB`` wins; otherwise the in-repo build dir (the cdylib
    cargo produced / carried forward to the Python image), per the
    sibling-checkout convention the Go binding uses.
    """
    override = os.environ.get("ANGZARR_ROUTER_LIB")
    if override:
        return override
    base = os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "target", "debug"
    )
    for name in (
        "libangzarr_router_ffi.so",
        "libangzarr_router_ffi.dylib",
        "angzarr_router_ffi.dll",
    ):
        candidate = os.path.join(base, name)
        if os.path.exists(candidate):
            return candidate
    raise OSError("router-ffi library not found; build it or set ANGZARR_ROUTER_LIB")


lib = ffi.dlopen(_library_path())
