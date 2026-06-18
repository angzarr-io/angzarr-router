"""The Python binding core: registration API, dispatch marshaling, and the
single cffi callback trampoline over the router-ffi C ABI.

Shaped like the Go binding (and the engine before it) so the unit-6 code
generator can target it with minimal emitter changes:
``Rebuilder``/``apply``/``with_snapshot`` and
``AggregateDispatch``/``on_command``/``on_rejected``. Host state never
crosses the FFI — it lives in a per-dispatch session reached from callbacks
via an ``ffi.new_handle`` parked in ``host_ctx`` (the cffi analog of Go's
``cgo.Handle``).
"""

from __future__ import annotations

import enum
import threading
from dataclasses import dataclass, field
from typing import Callable, Optional

from google.protobuf import any_pb2
from google.rpc import error_details_pb2, status_pb2

from ._abi import ffi, lib
from .gen.io.angzarr.router.ffi.v1 import abi_pb2
from .gen.io.angzarr.v1 import command_handler_pb2, types_pb2

# --- ABI status codes (mirror crates/router-ffi/src/abi.rs) ---
_STATUS_OK = 0  # success with a payload in `out`
_STATUS_OK_EMPTY = 1  # success, handler emitted nothing

# The code an unclassified handler failure surfaces as — the binding's job
# to classify, mirroring the Go binding and client-* engines.
CODE_UNHANDLED_HANDLER_ERROR = "UNHANDLED_HANDLER_ERROR"

# The reverse-DNS error domain on every ErrorInfo the boundary emits
# (distinct from the io.angzarr proto package — see plan §1).
ERROR_INFO_DOMAIN = "angzarr.io"


class GrpcCode(enum.IntEnum):
    """The numeric gRPC status codes carried with a coded error. Plain ints
    so the binding depends only on the protobuf runtime, not grpcio."""

    INVALID_ARGUMENT = 3
    NOT_FOUND = 5
    FAILED_PRECONDITION = 9
    UNIMPLEMENTED = 12
    INTERNAL = 13
    DATA_LOSS = 15


class CodedError(Exception):
    """A stable cross-language coded failure. A handler raises one (via
    :func:`reject`) to fail a command with a code like ``VALUE_NOT_POSITIVE``;
    the binding also raises one when decoding a coded failure the core
    returned (``NO_HANDLER_REGISTERED``, ``PERSISTED_EVENT_CORRUPT``, …). It
    crosses the FFI as ``google.rpc.Status`` carrying a
    ``google.rpc.ErrorInfo``."""

    def __init__(
        self,
        code: str = "",
        message: str = "",
        grpc: int = GrpcCode.INTERNAL,
        extras: Optional[dict[str, str]] = None,
    ):
        self.code = code
        self.message = message
        self.grpc = int(grpc)
        self.extras = dict(extras) if extras else {}
        super().__init__(f"{code}: {message}" if code else message)


def reject(code: str, message: str) -> CodedError:
    """Build an invalid-argument business rejection — the common shape a
    command handler raises to reject a command with a coded reason."""
    return CodedError(code=code, message=message, grpc=GrpcCode.INVALID_ARGUMENT)


@dataclass
class CommandContext:
    """The historical-state evidence a handler sees. Host state never
    crosses the FFI, so the core reconstructs this from the prior-events book
    and hands it back."""

    next_sequence: int = 0
    had_prior_events: bool = False


# Thunk shapes (host-supplied business logic):
#   applier:   (state, payload: Any) -> None            (folds; raises on corrupt)
#   command:   (cmd: Any, state, cctx) -> EventBook|None (raises CodedError to reject)
#   rejection: (notification, rejection, state, cctx) -> BusinessResponse|None
ApplierThunk = Callable[[object, any_pb2.Any], None]
CommandThunk = Callable[[any_pb2.Any, object, CommandContext], Optional[object]]
RejectionThunk = Callable[[object, object, object, CommandContext], Optional[object]]


@dataclass
class Rebuilder:
    """Folds an aggregate's prior events (and optional snapshot) into state
    before a command runs."""

    factory: Callable[[], object]
    appliers: dict[str, ApplierThunk] = field(default_factory=dict)
    snapshot: Optional[ApplierThunk] = None

    def apply(self, full_name: str, thunk: ApplierThunk) -> "Rebuilder":
        """Register an applier for one fully-qualified event type."""
        self.appliers[full_name] = thunk
        return self

    def with_snapshot(self, thunk: ApplierThunk) -> "Rebuilder":
        """Register the snapshot loader that seeds state before pages."""
        self.snapshot = thunk
        return self


@dataclass
class AggregateDispatch:
    """One aggregate component's registration: name, domain, rebuilder,
    command handlers, and ordered rejection compensators."""

    name: str
    domain: str
    rebuilder: Rebuilder
    commands: dict[str, CommandThunk] = field(default_factory=dict)
    rejections: dict[str, list[RejectionThunk]] = field(default_factory=dict)

    def on_command(self, full_name: str, thunk: CommandThunk) -> "AggregateDispatch":
        """Register a handler for one fully-qualified command type."""
        self.commands[full_name] = thunk
        return self

    def on_rejected(self, fq_command: str, thunk: RejectionThunk) -> "AggregateDispatch":
        """Append a compensator for one fully-qualified command type; repeated
        calls register an ordered fan-out."""
        self.rejections.setdefault(fq_command, []).append(thunk)
        return self


# --- error model: CodedError <-> google.rpc.Status bytes ---


def _build_status_bytes(grpc: int, message: str, code: str, extras: Optional[dict]) -> bytes:
    """Serialize a coded failure as google.rpc.Status bytes carrying an
    ErrorInfo detail — the exact shape the core decodes (and gRPC puts on the
    wire). ErrorInfo Any uses the type.googleapis.com prefix the ABI pins."""
    info = error_details_pb2.ErrorInfo(reason=code, domain=ERROR_INFO_DOMAIN, metadata=extras or {})
    any_info = any_pb2.Any()
    any_info.Pack(info)
    status = status_pb2.Status(code=int(grpc), message=message, details=[any_info])
    return status.SerializeToString()


def _error_status(exc: BaseException) -> tuple[bytes, int]:
    """Map a handler exception to (Status bytes, negative gRPC code): a
    CodedError keeps its code; any other exception is an unclassified failure
    → UNHANDLED_HANDLER_ERROR."""
    if isinstance(exc, CodedError):
        grpc = exc.grpc or GrpcCode.INVALID_ARGUMENT
        return _build_status_bytes(grpc, exc.message, exc.code, exc.extras), -int(grpc)
    return (
        _build_status_bytes(GrpcCode.INTERNAL, str(exc), CODE_UNHANDLED_HANDLER_ERROR, None),
        -int(GrpcCode.INTERNAL),
    )


def _decode_status(data: Optional[bytes], ret: int) -> CodedError:
    """Turn google.rpc.Status bytes (with an ErrorInfo detail) back into a
    CodedError. ``ret`` (the negative callback/dispatch return) is the gRPC
    fallback when the bytes are absent or undecodable."""
    code = ""
    message = ""
    grpc = -ret
    extras: dict[str, str] = {}
    if data:
        status = status_pb2.Status()
        try:
            status.ParseFromString(data)
        except Exception:
            return CodedError(grpc=grpc)
        message = status.message
        if status.code != 0:
            grpc = status.code
        for detail in status.details:
            if detail.Is(error_details_pb2.ErrorInfo.DESCRIPTOR):
                info = error_details_pb2.ErrorInfo()
                detail.Unpack(info)
                code = info.reason
                extras = dict(info.metadata)
                break
    return CodedError(code=code, message=message, grpc=grpc, extras=extras)


# --- session + type-erased invokers (mirror the Go binding) ---


class _Session:
    """One dispatch's host-side state object, reached from callbacks via the
    host_ctx handle. State never crosses to Rust; it lives here and is
    created lazily by the first callback (all callbacks in one dispatch belong
    to the same aggregate, so the factory is consistent)."""

    __slots__ = ("router", "_state", "_has_state")

    def __init__(self, router: "Router"):
        self.router = router
        self._state: object = None
        self._has_state = False

    def ensure_state(self, factory: Callable[[], object]) -> object:
        if not self._has_state:
            self._state = factory()
            self._has_state = True
        return self._state


# An invoker bridges a callback_id to a registered typed thunk: it receives
# the live session and the marshaled inputs and returns (out_bytes, status).
# Thunk exceptions are NOT caught here — the trampoline catches them once.
Invoker = Callable[["_Session", str, bytes, bytes], tuple[Optional[bytes], int]]


def _applier_invoker(factory, thunk: ApplierThunk) -> Invoker:
    def inv(session, type_url, payload, _aux):
        state = session.ensure_state(factory)
        thunk(state, any_pb2.Any(type_url=type_url, value=payload))
        return None, _STATUS_OK

    return inv


def _command_invoker(factory, thunk: CommandThunk) -> Invoker:
    def inv(session, type_url, payload, aux):
        cax = abi_pb2.CommandContextAux()
        cax.ParseFromString(aux)
        cctx = CommandContext(
            next_sequence=cax.next_sequence, had_prior_events=cax.had_prior_events
        )
        state = session.ensure_state(factory)
        book = thunk(any_pb2.Any(type_url=type_url, value=payload), state, cctx)
        if book is None:
            return None, _STATUS_OK_EMPTY
        return book.SerializeToString(), _STATUS_OK

    return inv


def _rejection_invoker(factory, thunk: RejectionThunk) -> Invoker:
    def inv(session, _type_url, _payload, aux):
        rax = abi_pb2.RejectionAux()
        rax.ParseFromString(aux)
        notification = types_pb2.Notification()
        notification.ParseFromString(rax.notification)
        rejection = types_pb2.RejectionNotification()
        rejection.ParseFromString(rax.rejection)
        cctx = CommandContext(
            next_sequence=rax.cctx.next_sequence, had_prior_events=rax.cctx.had_prior_events
        )
        state = session.ensure_state(factory)
        resp = thunk(notification, rejection, state, cctx)
        if resp is None:
            return None, _STATUS_OK_EMPTY
        return resp.SerializeToString(), _STATUS_OK

    return inv


# --- the single cffi callback trampoline ---


def _c_bytes(ptr, n) -> bytes:
    """Copy a router-owned input buffer (valid only for this callback) into
    Python bytes."""
    if ptr == ffi.NULL or n == 0:
        return b""
    return bytes(ffi.buffer(ptr, n))


def _write_out(out, data: Optional[bytes]) -> None:
    """Fill a router-allocated out buffer (host allocates via
    angzarr_buf_alloc; the router consumes and frees it). Empty leaves
    out null/zero."""
    if out == ffi.NULL:
        return
    if not data:
        out.data = ffi.NULL
        out.len = 0
        return
    ptr = lib.angzarr_buf_alloc(len(data))
    ffi.memmove(ptr, data, len(data))
    out.data = ptr
    out.len = len(data)


@ffi.callback("angzarr_cb")
def _trampoline(
    host_ctx, callback_id, type_url, type_url_len, payload, payload_len, aux, aux_len, out
):
    """The single C-visible gateway the core calls for every host callback.
    Recovers the dispatch session from host_ctx, routes by callback_id to the
    registered invoker, and writes the response into out. A Python exception
    is caught and surfaced as a coded failure — it must never unwind across
    the boundary into Rust."""
    try:
        session = ffi.from_handle(host_ctx)
        inv = session.router._registry.get(int(callback_id))
        if inv is None:
            data, code = _error_status(
                CodedError(
                    code=CODE_UNHANDLED_HANDLER_ERROR,
                    message=f"no host callback registered for id {int(callback_id)}",
                    grpc=GrpcCode.INTERNAL,
                )
            )
            _write_out(out, data)
            return code
        data, status = inv(
            session,
            _c_bytes(type_url, type_url_len).decode("utf-8"),
            _c_bytes(payload, payload_len),
            _c_bytes(aux, aux_len),
        )
        _write_out(out, data)
        return status
    except Exception as exc:  # noqa: BLE001 — boundary guard: nothing crosses into Rust
        data, code = _error_status(exc)
        _write_out(out, data)
        return code


def _as_u8(buf: bytes):
    """A read-only uint8_t* view over Python bytes (no copy). The caller must
    keep ``buf`` alive for the duration of the C call."""
    if not buf:
        return ffi.NULL
    return ffi.cast("uint8_t*", ffi.from_buffer(buf))


def _consume_out(out) -> bytes:
    """Copy a router-allocated out buffer into Python bytes and release it
    (the dispatch out is router-owned)."""
    if out.data == ffi.NULL or out.len == 0:
        return b""
    data = bytes(ffi.buffer(out.data, out.len))
    lib.angzarr_buf_release(out.data, out.len)
    out.data = ffi.NULL
    out.len = 0
    return data


class Router:
    """Wraps the Rust core router plus the Python-side callback registry the
    trampoline routes through. Registration is not safe for concurrent use;
    concurrent :meth:`dispatch` is — each dispatch parks its own state in a
    host_ctx the core isolates."""

    def __init__(self):
        self._ptr = lib.angzarr_router_new()
        self._registry: dict[int, Invoker] = {}
        self._next_id = 0
        self._lock = threading.Lock()

    def close(self) -> None:
        """Free the underlying Rust router. Safe to call once."""
        if self._ptr is not None:
            lib.angzarr_router_free(self._ptr)
            self._ptr = None

    def __enter__(self) -> "Router":
        return self

    def __exit__(self, *_exc) -> None:
        self.close()

    def _assign(self, inv: Invoker) -> int:
        self._next_id += 1
        self._registry[self._next_id] = inv
        return self._next_id

    def register_aggregate(self, dispatch: AggregateDispatch) -> None:
        """Register one aggregate component: assign callback ids to every
        thunk, serialize the AggregateDescriptor, and hand it to the core with
        the shared trampoline."""
        with self._lock:
            factory = dispatch.rebuilder.factory
            desc = abi_pb2.AggregateDescriptor(name=dispatch.name, domain=dispatch.domain)

            for fq, thunk in dispatch.rebuilder.appliers.items():
                cid = self._assign(_applier_invoker(factory, thunk))
                desc.appliers.append(abi_pb2.CallbackEntry(fq_type=fq, callback_id=cid))
            if dispatch.rebuilder.snapshot is not None:
                desc.snapshot_callback_id = self._assign(
                    _applier_invoker(factory, dispatch.rebuilder.snapshot)
                )
            for fq, thunk in dispatch.commands.items():
                cid = self._assign(_command_invoker(factory, thunk))
                desc.commands.append(abi_pb2.CallbackEntry(fq_type=fq, callback_id=cid))
            for fq, thunks in dispatch.rejections.items():
                entry = abi_pb2.RejectionEntry(fq_command_type=fq)
                for thunk in thunks:
                    entry.callback_ids.append(self._assign(_rejection_invoker(factory, thunk)))
                desc.rejections.append(entry)

            desc_bytes = desc.SerializeToString()
            ret = lib.angzarr_router_register_aggregate(
                self._ptr, _as_u8(desc_bytes), len(desc_bytes), _trampoline
            )
            if ret != 0:
                raise _decode_status(None, ret)

    def dispatch(self, contextual_command) -> object:
        """Run one ContextualCommand through the core and return the
        BusinessResponse, or raise a CodedError decoded from the core's
        failure."""
        req = contextual_command.SerializeToString()
        # The session is reached from callbacks via this handle; the core holds
        # it only for the duration of this synchronous call. `handle` must stay
        # referenced until dispatch returns.
        session = _Session(self)
        handle = ffi.new_handle(session)
        out = ffi.new("angzarr_buf*")
        ret = lib.angzarr_router_dispatch(self._ptr, handle, _as_u8(req), len(req), out)
        resp_bytes = _consume_out(out)
        if ret == 0:
            resp = command_handler_pb2.BusinessResponse()
            if resp_bytes:
                resp.ParseFromString(resp_bytes)
            return resp
        raise _decode_status(resp_bytes, ret)


def abi_version() -> int:
    """The ABI version the linked router-ffi exposes. Bindings check it so a
    binding and a router-ffi artifact that have drifted refuse each other."""
    return int(lib.angzarr_abi_version())
