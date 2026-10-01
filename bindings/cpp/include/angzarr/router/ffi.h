#pragma once

// The raw C-ABI of the router-ffi staticlib (libangzarr_router_ffi.a), linked
// directly. The symbols have C linkage (extern "C") even though declared inside
// a namespace. AngzarrBuf is the byte buffer crossing the boundary; AngzarrCb is
// the single host-callback gateway signature.

#include <cstddef>
#include <cstdint>

namespace angzarr::router::ffi {

extern "C" {

struct AngzarrBuf {
  uint8_t* data;
  size_t len;
};

using AngzarrCb = int32_t (*)(void* host_ctx, uint64_t callback_id, const uint8_t* type_url,
                              size_t type_url_len, const uint8_t* payload, size_t payload_len,
                              const uint8_t* aux, size_t aux_len, AngzarrBuf* out);

uint32_t angzarr_abi_version();
uint8_t* angzarr_buf_alloc(size_t len);
void angzarr_buf_release(uint8_t* ptr, size_t len);
void* angzarr_router_new();
void angzarr_router_free(void* r);
int32_t angzarr_router_register_aggregate(void* r, const uint8_t* descriptor, size_t len,
                                          AngzarrCb cb);
int32_t angzarr_router_register_projector(void* r, const uint8_t* descriptor, size_t len,
                                          AngzarrCb cb);
int32_t angzarr_router_register_saga(void* r, const uint8_t* descriptor, size_t len, AngzarrCb cb);
int32_t angzarr_router_register_process_manager(void* r, const uint8_t* descriptor, size_t len,
                                                AngzarrCb cb);
int32_t angzarr_router_dispatch(void* r, void* host_ctx, const uint8_t* request, size_t len,
                                AngzarrBuf* out);
int32_t angzarr_router_dispatch_projector(void* r, void* host_ctx, const uint8_t* request,
                                          size_t len, AngzarrBuf* out);
int32_t angzarr_router_dispatch_saga(void* r, void* host_ctx, const uint8_t* request, size_t len,
                                     AngzarrBuf* out);
int32_t angzarr_router_dispatch_process_manager(void* r, void* host_ctx, const uint8_t* request,
                                                size_t len, AngzarrBuf* out);
// FactRequest bytes in, EventBook bytes (the facts to record) out; routed to
// the aggregate claiming the facts' cover domain.
int32_t angzarr_router_dispatch_fact(void* r, void* host_ctx, const uint8_t* request, size_t len,
                                     AngzarrBuf* out);
// io.angzarr.router.ffi.v1.ReplayCall bytes in, ReplayResponse bytes out;
// routed to the aggregate claiming ReplayCall.domain.
int32_t angzarr_router_dispatch_replay(void* r, void* host_ctx, const uint8_t* request, size_t len,
                                       AngzarrBuf* out);

}  // extern "C"

// The router-ffi ABI version this binding is written against.
inline constexpr uint32_t kAbiVersion = 2;

inline constexpr int32_t kStatusOk = 0;
inline constexpr int32_t kStatusOkEmpty = 1;

}  // namespace angzarr::router::ffi
