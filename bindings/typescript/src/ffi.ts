// The raw C-ABI layer over the router-ffi cdylib, via koffi. Holds the 15
// exported downcalls, the AngzarrBuf layout, and the single registered callback
// trampoline the core calls for every host callback.
//
// Memory ownership is symmetric, copy-at-the-boundary: a callback fills the
// router-allocated `out` (via angzarr_buf_alloc); a dispatch response is
// router-allocated and released here (angzarr_buf_release). The trampoline never
// lets an exception unwind across the boundary — the per-dispatch Session it
// reaches is parked in a module-level map whose integer id the core carries as
// host_ctx. host_ctx is a void* the cdylib treats opaquely; it is declared
// uintptr_t (pointer-sized, passed exactly as a pointer) because koffi accepts
// only a real pointer object for a void* argument, never an integer id.

import koffi from "koffi";

import { type Session } from "./session";
import { unhandled } from "./codedError";
import { errorResult, type Outcome } from "./statuses";

/** The router-ffi ABI version this binding speaks. */
export const ABI_VERSION = 3;
const LIB_ENV = "ANGZARR_ROUTER_LIB";

function libraryPath(): string {
  const path = process.env[LIB_ENV];
  if (!path) {
    throw new Error(
      `router-ffi cdylib path not set — set ${LIB_ENV} to libangzarr_router_ffi.so`,
    );
  }
  return path;
}

const lib = koffi.load(libraryPath());

// AngzarrBuf { *mut u8 data; usize len } — the byte buffer crossing the C ABI.
const AngzarrBuf = koffi.struct("AngzarrBuf", {
  data: "uint8_t*",
  len: "size_t",
});

// The single host-callback gateway signature; host_ctx is the opaque session id.
const AngzarrCb = koffi.proto(
  "int32_t AngzarrCb(uintptr_t host_ctx, uint64_t callback_id, uint8_t* type_url, size_t type_url_len, uint8_t* payload, size_t payload_len, uint8_t* aux, size_t aux_len, _Out_ AngzarrBuf* out)",
);

// --- downcalls ---------------------------------------------------------------

const angzarr_abi_version = lib.func("uint32_t angzarr_abi_version()");
const angzarr_buf_alloc = lib.func("uint8_t* angzarr_buf_alloc(size_t len)");
const angzarr_buf_release = lib.func(
  "void angzarr_buf_release(uint8_t* ptr, size_t len)",
);
const angzarr_router_new = lib.func("void* angzarr_router_new()");
const angzarr_router_free = lib.func("void angzarr_router_free(void* r)");

const registerFns = {
  aggregate: lib.func(
    "int32_t angzarr_router_register_aggregate(void* r, uint8_t* descriptor, size_t len, void* cb)",
  ),
  projector: lib.func(
    "int32_t angzarr_router_register_projector(void* r, uint8_t* descriptor, size_t len, void* cb)",
  ),
  saga: lib.func(
    "int32_t angzarr_router_register_saga(void* r, uint8_t* descriptor, size_t len, void* cb)",
  ),
  processManager: lib.func(
    "int32_t angzarr_router_register_process_manager(void* r, uint8_t* descriptor, size_t len, void* cb)",
  ),
};

const dispatchFns = {
  aggregate: lib.func(
    "int32_t angzarr_router_dispatch(void* r, uintptr_t host_ctx, uint8_t* request, size_t len, _Out_ AngzarrBuf* out)",
  ),
  projector: lib.func(
    "int32_t angzarr_router_dispatch_projector(void* r, uintptr_t host_ctx, uint8_t* request, size_t len, _Out_ AngzarrBuf* out)",
  ),
  saga: lib.func(
    "int32_t angzarr_router_dispatch_saga(void* r, uintptr_t host_ctx, uint8_t* request, size_t len, _Out_ AngzarrBuf* out)",
  ),
  processManager: lib.func(
    "int32_t angzarr_router_dispatch_process_manager(void* r, uintptr_t host_ctx, uint8_t* request, size_t len, _Out_ AngzarrBuf* out)",
  ),
  fact: lib.func(
    "int32_t angzarr_router_dispatch_fact(void* r, uintptr_t host_ctx, uint8_t* request, size_t len, _Out_ AngzarrBuf* out)",
  ),
  replay: lib.func(
    "int32_t angzarr_router_dispatch_replay(void* r, uintptr_t host_ctx, uint8_t* request, size_t len, _Out_ AngzarrBuf* out)",
  ),
};

/** Refuses a router-ffi library whose ABI version is not {@link ABI_VERSION}. */
export function checkAbiVersion(actual: number): void {
  if (actual !== ABI_VERSION) {
    throw new Error(
      `router-ffi ABI version mismatch: expected ${ABI_VERSION}, actual ${actual}`,
    );
  }
}

// --- session registry (host_ctx ↔ Session) -----------------------------------

const sessions = new Map<number, Session>();
let nextHostCtx = 0;

function registerSession(session: Session): number {
  const id = ++nextHostCtx;
  sessions.set(id, session);
  return id;
}

function unregisterSession(id: number): void {
  sessions.delete(id);
}

// --- buffer marshalling ------------------------------------------------------

function readBytes(ptr: unknown, len: number): Uint8Array {
  if (!ptr || len === 0) {
    return new Uint8Array(0);
  }
  return koffi.decode(ptr, koffi.array("uint8_t", len)) as Uint8Array;
}

const decoder = new TextDecoder();

function writeOut(out: unknown, bytes: Uint8Array | null): void {
  if (!bytes || bytes.length === 0) {
    koffi.encode(out, AngzarrBuf, { data: null, len: 0 });
    return;
  }
  const data = angzarr_buf_alloc(bytes.length);
  koffi.encode(data, koffi.array("uint8_t", bytes.length), bytes);
  koffi.encode(out, AngzarrBuf, { data, len: bytes.length });
}

function consumeOut(out: unknown): Uint8Array {
  const buf = koffi.decode(out, AngzarrBuf) as {
    data: unknown;
    len: number | bigint;
  };
  const len = Number(buf.len);
  if (!buf.data || len === 0) {
    return new Uint8Array(0);
  }
  const bytes = (
    koffi.decode(buf.data, koffi.array("uint8_t", len)) as Uint8Array
  ).slice();
  angzarr_buf_release(buf.data, len);
  return bytes;
}

// --- the trampoline (called by the core, on the dispatching thread) -----------

/** Routes one host callback to the session parked under `hostCtx`, returning
 * the response bytes + ABI status the trampoline hands back to the core. */
export function hostCallback(
  hostCtx: number | bigint,
  callbackId: number | bigint,
  typeUrl: string,
  payload: Uint8Array,
  aux: Uint8Array,
): Outcome {
  const session = sessions.get(Number(hostCtx));
  if (!session) {
    return errorResult(
      unhandled(`no dispatch session for host_ctx ${hostCtx}`),
    );
  }
  return session.handleCallback(Number(callbackId), typeUrl, payload, aux);
}

const callback = koffi.register(
  (
    hostCtx: number | bigint,
    callbackId: bigint,
    typeUrl: unknown,
    typeUrlLen: number | bigint,
    payload: unknown,
    payloadLen: number | bigint,
    aux: unknown,
    auxLen: number | bigint,
    out: unknown,
  ): number => {
    try {
      const result = hostCallback(
        hostCtx,
        callbackId,
        decoder.decode(readBytes(typeUrl, Number(typeUrlLen))),
        readBytes(payload, Number(payloadLen)),
        readBytes(aux, Number(auxLen)),
      );
      writeOut(out, result.response);
      return result.status;
    } catch (e) {
      // Defense in depth: handleCallback owns the firewall and should never
      // throw. If it somehow does, fail closed without unwinding into Rust.
      const failed = errorResult(unhandled(`host callback failed: ${e}`));
      try {
        writeOut(out, failed.response);
      } catch {
        // `out` stays the core's empty buffer; the status still classifies it.
      }
      return failed.status;
    }
  },
  koffi.pointer(AngzarrCb),
);

// --- public surface ----------------------------------------------------------

export interface Dispatched {
  response: Uint8Array;
  status: number;
}

export type Surface =
  | "aggregate"
  | "projector"
  | "saga"
  | "processManager"
  | "fact"
  | "replay";

/** The registration entry points (one per component kind). */
export type RegisterSurface = keyof typeof registerFns;

export const Ffi = {
  STATUS_OK: 0,
  STATUS_OK_EMPTY: 1,

  /** The ABI version the loaded router-ffi library reports. */
  abiVersion(): number {
    return Number(angzarr_abi_version());
  },

  init(): void {
    checkAbiVersion(Ffi.abiVersion());
  },

  routerNew(): unknown {
    return angzarr_router_new();
  },

  routerFree(ptr: unknown): void {
    angzarr_router_free(ptr);
  },

  register(
    surface: RegisterSurface,
    ptr: unknown,
    descriptor: Uint8Array,
  ): number {
    return registerFns[surface](ptr, descriptor, descriptor.length, callback);
  },

  dispatch(
    surface: Surface,
    ptr: unknown,
    session: Session,
    request: Uint8Array,
  ): Dispatched {
    const hostCtx = registerSession(session);
    const out = koffi.alloc(AngzarrBuf, 1);
    try {
      const status = dispatchFns[surface](
        ptr,
        hostCtx,
        request,
        request.length,
        out,
      );
      return { response: consumeOut(out), status };
    } finally {
      unregisterSession(hostCtx);
    }
  },
};
