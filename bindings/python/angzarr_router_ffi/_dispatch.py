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
from collections.abc import Callable, Iterable, Iterator
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from google.protobuf import any_pb2
from google.protobuf.message import DecodeError, Message
from google.rpc import error_details_pb2, status_pb2

from ._abi import ffi, lib
from .gen.io.angzarr.router.ffi.v1 import abi_pb2
from .gen.io.angzarr.v1 import (
    command_handler_pb2,
    process_manager_pb2,
    saga_pb2,
    types_pb2,
)

if TYPE_CHECKING:
    from typing_extensions import Self

# --- ABI status codes (mirror crates/router-ffi/src/abi.rs) ---
_STATUS_OK = 0  # success with a payload in `out`
_STATUS_OK_EMPTY = 1  # success, handler emitted nothing

# The code an unclassified handler failure surfaces as — the binding's job
# to classify, mirroring the Go binding and client-* engines.
CODE_UNHANDLED_HANDLER_ERROR = "UNHANDLED_HANDLER_ERROR"

# The reverse-DNS error domain on every ErrorInfo the boundary emits
# (distinct from the io.angzarr proto package — see plan §1).
ERROR_INFO_DOMAIN = "angzarr.io"

# The code AnyDecodeError carries.
CODE_ANY_DECODE_FAILED = "ANY_DECODE_FAILED"

# The framework's Any type-URL convention: a bare "/" followed by the
# fully-qualified message name (NOT the type.googleapis.com prefix Any.Pack
# defaults to). The core keys event/command dispatch on it.
_FRAMEWORK_ANY_PREFIX = "/"


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
        extras: dict[str, str] | None = None,
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


def any_decode_error(type_url: str, cause: BaseException) -> CodedError:
    """Report that a google.protobuf.Any payload failed to parse to its
    expected type. Generated dispatch thunks raise it when a command or event
    Any cannot be decoded — a malformed payload is an invalid argument, not a
    handler bug."""
    return CodedError(
        code=CODE_ANY_DECODE_FAILED,
        message=f"decode Any {type_url!r}: {cause}",
        grpc=GrpcCode.INVALID_ARGUMENT,
        extras={"type_url": type_url},
    )


def pack(msg) -> any_pb2.Any:
    """Wrap a message in a google.protobuf.Any using the framework's bare-"/"
    type-URL convention. Generated typed-emit wiring uses it to build an
    EventBook from the typed events a command handler returns."""
    return any_pb2.Any(
        type_url=_FRAMEWORK_ANY_PREFIX + msg.DESCRIPTOR.full_name,
        value=msg.SerializeToString(),
    )


@dataclass
class CommandContext:
    """The historical-state evidence a handler sees, and the cover it is
    handling. Host state never crosses the FFI, so the core reconstructs this
    from the prior-events book and hands it back. ``cover`` is the command's
    (or notification delivery's) cover: the aggregate's own domain and root,
    None when the command carried none."""

    next_sequence: int = 0
    had_prior_events: bool = False
    cover: types_pb2.Cover | None = None


@dataclass(frozen=True)
class PageContext:
    """Where the event or command a handler is handling sits: the cover of
    its book (None when the book carries none) and the page's explicit
    sequence (0 when the page carries none, and for commands, sagas and
    process-manager triggers, whose callbacks carry no page sequence).
    Projector folds and aggregate / process-manager appliers see the
    folded event's own book cover and page sequence."""

    cover: types_pb2.Cover | None = None
    sequence: int = 0


_CURRENT_PAGE: ContextVar[PageContext | None] = ContextVar(
    "angzarr_router_ffi_current_page", default=None
)


@contextmanager
def _handling(ctx: PageContext) -> Iterator[None]:
    """Make ``ctx`` the current page context while the block runs."""
    token = _CURRENT_PAGE.set(ctx)
    try:
        yield
    finally:
        _CURRENT_PAGE.reset(token)


def _cover_of(msg, name: str) -> types_pb2.Cover | None:
    """``msg``'s cover field ``name``, or None when it is unset."""
    return getattr(msg, name) if msg.HasField(name) else None


def current_page() -> PageContext:
    """The page context of the handler running on this thread. A dispatch
    sets it for every callback it makes: an aggregate command, compensation
    or undo handler sees its command's cover, a fact handler the facts'
    cover, a saga handler the source cover, a process-manager handler the
    trigger cover, a projector fold or an aggregate / process-manager applier
    its event's book cover and page sequence.
    Raises RuntimeError outside a dispatch."""
    ctx = _CURRENT_PAGE.get()
    if ctx is None:
        raise RuntimeError("no angzarr dispatch is being handled on this thread")
    return ctx


def current_cover() -> types_pb2.Cover:
    """The cover of the book the running handler is handling (see
    :func:`current_page`). Raises RuntimeError outside a dispatch or when that
    book carries no cover."""
    cover = current_page().cover
    if cover is None:
        raise RuntimeError("the book being handled carries no cover")
    return cover


def _command_context(cax) -> CommandContext:
    """The CommandContext a CommandContextAux stands for."""
    return CommandContext(
        next_sequence=cax.next_sequence,
        had_prior_events=cax.had_prior_events,
        cover=_cover_of(cax, "cover"),
    )


# Thunk shapes (host-supplied business logic):
#   applier:   (state, payload: Any) -> None            (folds; raises on corrupt)
#   context applier: (state, payload: Any, ctx: PageContext) -> None
#   command:   (cmd: Any, state, cctx) -> EventBook|None (raises CodedError to reject)
#   rejection: (notification, rejection, state, cctx) -> BusinessResponse|None
#   undo:      (notification, compensate, state, cctx) -> BusinessResponse|None
#   fact:      (fact: Any, state) -> Message|Any|None    (the fact to record;
#                                                         None = unchanged)
ApplierThunk = Callable[[object, any_pb2.Any], None]
ApplierContextThunk = Callable[[object, any_pb2.Any, PageContext], None]
CommandThunk = Callable[[any_pb2.Any, object, CommandContext], object | None]
RejectionThunk = Callable[[object, object, object, CommandContext], object | None]
UndoThunk = Callable[[object, object, object, CommandContext], object | None]
FactThunk = Callable[[any_pb2.Any, object], object | None]


@dataclass
class Rebuilder:
    """Folds an aggregate's prior events (and optional snapshot) into state
    before a command runs."""

    factory: Callable[[], object]
    appliers: dict[str, ApplierThunk] = field(default_factory=dict)
    snapshot: ApplierThunk | None = None

    def apply(self, full_name: str, thunk: ApplierThunk) -> Rebuilder:
        """Register an applier for one fully-qualified event type."""
        self.appliers[full_name] = thunk
        return self

    def apply_with_context(self, full_name: str, thunk: ApplierContextThunk) -> Rebuilder:
        """Register an applier for one fully-qualified event type that also
        receives the event's :class:`PageContext` (its book's cover and the
        page's sequence)."""

        def fold(state, event):
            thunk(state, event, current_page())

        self.appliers[full_name] = fold
        return self

    def with_snapshot(self, thunk: ApplierThunk) -> Rebuilder:
        """Register the snapshot loader that seeds state before pages."""
        self.snapshot = thunk
        return self


@dataclass
class AggregateDispatch:
    """One aggregate component's registration: name, domain, rebuilder,
    command handlers, ordered rejection compensators, undo handlers and fact
    handlers. An aggregate whose state (the rebuilder factory's product) is a
    protobuf message also supports Replay: the router packs its rebuilt
    state."""

    name: str
    domain: str
    rebuilder: Rebuilder
    commands: dict[str, CommandThunk] = field(default_factory=dict)
    rejections: dict[str, list[RejectionThunk]] = field(default_factory=dict)
    undoes: dict[str, UndoThunk] = field(default_factory=dict)
    facts: dict[str, FactThunk] = field(default_factory=dict)

    def on_command(self, full_name: str, thunk: CommandThunk) -> AggregateDispatch:
        """Register a handler for one fully-qualified command type."""
        self.commands[full_name] = thunk
        return self

    def on_rejected(self, compensates: str, thunk: RejectionThunk) -> AggregateDispatch:
        """Append a compensator under a ``compensates`` entry: the rejected
        command's fully-qualified type (``"fq.Type"``, sent to any domain) or
        ``"domain:fq.Type"`` (only when it was sent to that domain). Repeated
        calls register an ordered fan-out."""
        self.rejections.setdefault(compensates, []).append(thunk)
        return self

    def on_undo(self, fq_command: str, thunk: UndoThunk) -> AggregateDispatch:
        """Register the undo handler for a Compensate whose ``command_type``
        is ``fq_command`` (the fully-qualified type of the executed command to
        undo). The handler receives (notification, compensate, state, cctx)
        and returns a BusinessResponse, or None to record nothing. A
        Compensate with no undo handler fails NO_UNDO_HANDLER
        (UNIMPLEMENTED)."""
        self.undoes[fq_command] = thunk
        return self

    def on_fact(self, fq_fact: str, thunk: FactThunk) -> AggregateDispatch:
        """Register the fact handler for a fully-qualified fact (event) type.
        The handler receives (fact Any, rebuilt state) and returns the fact to
        record: a message (packed with the framework type-URL prefix), an
        Any, or None to record the fact unchanged. A fact cannot be refused."""
        self.facts[fq_fact] = thunk
        return self


# Projector thunk shapes:
#   event:   (state, event: Any) -> None             (folds; raises on corrupt)
#   context event: (state, event: Any, ctx: PageContext) -> None
#   finish:  (state, events: EventBook) -> Projection (packs the folded state)
#   unknown: (type_url: str) -> None                  (observes an unhandled type)
ProjectorEventThunk = Callable[[object, any_pb2.Any], None]
ProjectorContextThunk = Callable[[object, any_pb2.Any, PageContext], None]
ProjectorFinishThunk = Callable[[object, object], object]
ProjectorUnknownThunk = Callable[[str], None]


@dataclass
class ProjectorDispatch:
    """One projector component's registration: name, the domains it consumes
    (empty = all), fold handlers, an optional catch-all for unhandled types,
    and an optional finisher. Shaped like the core/Go API so the unit-6
    emitter targets it with minimal changes."""

    name: str
    factory: Callable[[], object]
    domains: list[str] = field(default_factory=list)
    events: dict[str, ProjectorEventThunk] = field(default_factory=dict)
    unknown: ProjectorUnknownThunk | None = None
    finisher: ProjectorFinishThunk | None = None

    def for_domains(self, *domains: str) -> ProjectorDispatch:
        """Restrict folding to books whose cover carries one of these domains.
        Unset (the default) consumes every domain."""
        self.domains = list(domains)
        return self

    def on_event(self, full_name: str, thunk: ProjectorEventThunk) -> ProjectorDispatch:
        """Register the fold thunk for a fully-qualified event type name."""
        self.events[full_name] = thunk
        return self

    def on_event_with_context(
        self, full_name: str, thunk: ProjectorContextThunk
    ) -> ProjectorDispatch:
        """Register a fold thunk for a fully-qualified event type that also
        receives the event's :class:`PageContext` (its book's cover and the
        page's sequence)."""

        def fold(state, event):
            thunk(state, event, current_page())

        self.events[full_name] = fold
        return self

    def on_unknown(self, thunk: ProjectorUnknownThunk) -> ProjectorDispatch:
        """Register a catch-all for events with no fold thunk."""
        self.unknown = thunk
        return self

    def finish(self, thunk: ProjectorFinishThunk) -> ProjectorDispatch:
        """Register the finisher that packs the folded instance into the wire
        Projection."""
        self.finisher = thunk
        return self


class Destinations:
    """The declared output domains of one saga or process manager (its command
    targets), in declaration order. Emitted commands carry no destination
    sequence: the router stamps their ``angzarr_deferred`` provenance from the
    triggering page, so a handler returns its commands as built."""

    __slots__ = ("_domains",)

    def __init__(self, domains: Iterable[str] | None = None):
        self._domains = list(domains) if domains else []

    def has(self, domain: str) -> bool:
        """Whether ``domain`` is a declared output domain."""
        return domain in self._domains

    def domains(self) -> list[str]:
        """The declared output domains, in declaration order."""
        return list(self._domains)


# Saga thunk shape (a saga is stateless — no state argument):
#   event: (event: Any, dests: Destinations, source_cover: Cover) -> (commands, events)
# dests are the saga's declared targets; source_cover is the source book's
# cover passed through whole so the saga can route emitted commands by the
# trigger's identity. The router stamps the commands deferred. Sagas receive
# no rejections.
SagaEventThunk = Callable[[any_pb2.Any, Destinations, object], tuple[list, list]]


@dataclass
class SagaDispatch:
    """One saga component's registration: name, the input domain it consumes,
    the domains it issues commands to (its declared output domains), and event
    handlers. A saga is stateless — no rebuilder, no state — and receives no
    rejections. Shaped like the core/Go API."""

    name: str
    input_domain: str
    targets: list[str] = field(default_factory=list)
    events: dict[str, SagaEventThunk] = field(default_factory=dict)

    def on_event(self, full_name: str, thunk: SagaEventThunk) -> SagaDispatch:
        """Register the translation thunk for a fully-qualified event type."""
        self.events[full_name] = thunk
        return self


# Process-manager thunk shapes (a PM is stateful — it sees rebuilt state):
#   event:     (event: Any, state, dests) -> ProcessManagerHandleResponse
#   cover event: (event: Any, state, dests, trigger_cover: Cover | None) ->
#                ProcessManagerHandleResponse
#   rejection: (notification, rejection, state) ->
#                ProcessManagerHandleResponse            (process events, commands,
#                                                         facts, escalation)
#              | (process_events, escalation | None)
#              | None                                    (nothing)
# dests are the PM's declared targets. The router stamps every command the
# PM returns (from an event handler or a compensator) deferred.
PMEventThunk = Callable[[any_pb2.Any, object, Destinations], object]
PMCoverEventThunk = Callable[[any_pb2.Any, object, Destinations, object], object]
PMRejectionThunk = Callable[[object, object, object], object]


@dataclass
class ProcessManagerDispatch:
    """One process-manager component's registration: name, its own domain, the
    rebuilder for its event-sourced state, its declared output domains
    (``targets``, its command targets), event handlers keyed by (input domain,
    FQ event type), and ordered rejection compensators keyed by ``compensates``
    entry. Shaped like the core/Go API."""

    name: str
    pm_domain: str
    rebuilder: Rebuilder
    targets: list[str] = field(default_factory=list)
    handlers: dict[str, dict[str, PMEventThunk]] = field(default_factory=dict)
    rejections: dict[str, list[PMRejectionThunk]] = field(default_factory=dict)

    def on_event(
        self, input_domain: str, full_name: str, thunk: PMEventThunk
    ) -> ProcessManagerDispatch:
        """Register the thunk for (input domain, fully-qualified event type)."""
        self.handlers.setdefault(input_domain, {})[full_name] = thunk
        return self

    def on_event_with_cover(
        self, input_domain: str, full_name: str, thunk: PMCoverEventThunk
    ) -> ProcessManagerDispatch:
        """Register a thunk for (input domain, fully-qualified event type)
        that also receives the trigger book's cover (None when it carries
        none)."""

        def handle(event, state, dests):
            return thunk(event, state, dests, current_page().cover)

        self.handlers.setdefault(input_domain, {})[full_name] = handle
        return self

    def on_rejected(self, compensates: str, thunk: PMRejectionThunk) -> ProcessManagerDispatch:
        """Append a compensator under a ``compensates`` entry: the rejected
        command's fully-qualified type (``"fq.Type"``, sent to any domain) or
        ``"domain:fq.Type"`` (only when it was sent to that domain). Repeated
        calls register an ordered fan-out (C-0042). The compensator may return
        a full ProcessManagerHandleResponse (its commands are kept and stamped
        deferred) or the ``(process_events, escalation)`` pair."""
        self.rejections.setdefault(compensates, []).append(thunk)
        return self


# --- error model: CodedError <-> google.rpc.Status bytes ---


def _build_status_bytes(grpc: int, message: str, code: str, extras: dict | None) -> bytes:
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


def _decode_status(data: bytes | None, ret: int) -> CodedError:
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
        except DecodeError:
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
    """One dispatch's host-side state, reached from callbacks via the host_ctx
    handle. State never crosses to Rust; it lives here, created lazily per
    component. One dispatch may run several components (co-resident process
    managers subscribed to the same trigger), so each component's state is
    keyed by the component key assigned at registration and is never shared
    with another component."""

    __slots__ = ("_states", "router")

    def __init__(self, router: Router):
        self.router = router
        self._states: dict[int, object] = {}

    def ensure_state(self, key: int, factory: Callable[[], object]) -> object:
        """The state for component ``key``, created by ``factory`` on first
        use within this dispatch."""
        if key not in self._states:
            self._states[key] = factory()
        return self._states[key]


# An invoker bridges a callback_id to a registered typed thunk: it receives
# the live session and the marshaled inputs and returns (out_bytes, status).
# Thunk exceptions are NOT caught here — the trampoline catches them once.
Invoker = Callable[[_Session, str, bytes, bytes], tuple[bytes | None, int]]


def _applier_invoker(key: int, factory, thunk: ApplierThunk) -> Invoker:
    # The applier aux is a ProjectorEventAux: the folded event's book cover
    # and page sequence, current while the applier runs.
    def inv(session, type_url, payload, aux):
        pax = abi_pb2.ProjectorEventAux()
        pax.ParseFromString(aux)
        state = session.ensure_state(key, factory)
        with _handling(PageContext(cover=_cover_of(pax, "cover"), sequence=pax.sequence)):
            thunk(state, any_pb2.Any(type_url=type_url, value=payload))
        return None, _STATUS_OK

    return inv


def _snapshot_invoker(key: int, factory, thunk: ApplierThunk) -> Invoker:
    def inv(session, type_url, payload, _aux):
        state = session.ensure_state(key, factory)
        thunk(state, any_pb2.Any(type_url=type_url, value=payload))
        return None, _STATUS_OK

    return inv


def _command_invoker(key: int, factory, thunk: CommandThunk) -> Invoker:
    def inv(session, type_url, payload, aux):
        cax = abi_pb2.CommandContextAux()
        cax.ParseFromString(aux)
        cctx = _command_context(cax)
        state = session.ensure_state(key, factory)
        with _handling(PageContext(cover=cctx.cover)):
            book = thunk(any_pb2.Any(type_url=type_url, value=payload), state, cctx)
        if book is None:
            return None, _STATUS_OK_EMPTY
        return book.SerializeToString(), _STATUS_OK

    return inv


def _rejection_invoker(key: int, factory, thunk: RejectionThunk) -> Invoker:
    def inv(session, _type_url, _payload, aux):
        rax = abi_pb2.RejectionAux()
        rax.ParseFromString(aux)
        notification = types_pb2.Notification()
        notification.ParseFromString(rax.notification)
        rejection = types_pb2.RejectionNotification()
        rejection.ParseFromString(rax.rejection)
        cctx = _command_context(rax.cctx)
        state = session.ensure_state(key, factory)
        with _handling(PageContext(cover=cctx.cover)):
            resp = thunk(notification, rejection, state, cctx)
        if resp is None:
            return None, _STATUS_OK_EMPTY
        return resp.SerializeToString(), _STATUS_OK

    return inv


def _undo_invoker(key: int, factory, thunk: UndoThunk) -> Invoker:
    def inv(session, _type_url, _payload, aux):
        uax = abi_pb2.UndoAux()
        uax.ParseFromString(aux)
        notification = types_pb2.Notification()
        notification.ParseFromString(uax.notification)
        compensate = types_pb2.Compensate()
        compensate.ParseFromString(uax.compensate)
        cctx = _command_context(uax.cctx)
        state = session.ensure_state(key, factory)
        with _handling(PageContext(cover=cctx.cover)):
            resp = thunk(notification, compensate, state, cctx)
        if resp is None:
            return None, _STATUS_OK_EMPTY
        return resp.SerializeToString(), _STATUS_OK

    return inv


def _fact_invoker(key: int, factory, thunk: FactThunk) -> Invoker:
    # The facts' cover is the dispatch-level page context Router.dispatch_fact
    # sets; the fact callback carries no aux.
    def inv(session, type_url, payload, _aux):
        state = session.ensure_state(key, factory)
        recorded = thunk(any_pb2.Any(type_url=type_url, value=payload), state)
        if recorded is None:
            return None, _STATUS_OK_EMPTY
        if not isinstance(recorded, any_pb2.Any):
            recorded = pack(recorded)
        return recorded.SerializeToString(), _STATUS_OK

    return inv


def _state_invoker(key: int, factory) -> Invoker:
    # Packs the component's rebuilt state for Replay.
    def inv(session, _type_url, _payload, _aux):
        return pack(session.ensure_state(key, factory)).SerializeToString(), _STATUS_OK

    return inv


def _projector_event_invoker(key: int, factory, thunk: ProjectorEventThunk) -> Invoker:
    def inv(session, type_url, payload, aux):
        pax = abi_pb2.ProjectorEventAux()
        pax.ParseFromString(aux)
        state = session.ensure_state(key, factory)
        with _handling(PageContext(cover=_cover_of(pax, "cover"), sequence=pax.sequence)):
            thunk(state, any_pb2.Any(type_url=type_url, value=payload))
        return None, _STATUS_OK

    return inv


def _projector_finish_invoker(key: int, factory, thunk: ProjectorFinishThunk) -> Invoker:
    def inv(session, _type_url, payload, _aux):
        # The core hands the EventBook over as the callback payload so the
        # finisher can carry its cover onto the Projection.
        book = types_pb2.EventBook()
        if payload:
            book.ParseFromString(payload)
        state = session.ensure_state(key, factory)
        projection = thunk(state, book)
        if projection is None:
            return None, _STATUS_OK_EMPTY
        return projection.SerializeToString(), _STATUS_OK

    return inv


def _projector_unknown_invoker(thunk: ProjectorUnknownThunk) -> Invoker:
    def inv(_session, type_url, _payload, _aux):
        thunk(type_url)
        return None, _STATUS_OK

    return inv


def _saga_event_invoker(targets: list[str], thunk: SagaEventThunk) -> Invoker:
    # Saga is stateless — the session's host state is untouched. The event
    # thunk sees the saga's declared targets and returns a SagaResponse.
    def inv(_session, type_url, payload, aux):
        sax = abi_pb2.SagaEventAux()
        sax.ParseFromString(aux)
        with _handling(PageContext(cover=_cover_of(sax, "source_cover"))):
            commands, events = thunk(
                any_pb2.Any(type_url=type_url, value=payload),
                Destinations(targets),
                sax.source_cover,
            )
        resp = saga_pb2.SagaResponse(commands=commands, events=events)
        return resp.SerializeToString(), _STATUS_OK

    return inv


def _pm_event_invoker(key: int, factory, targets: list[str], thunk: PMEventThunk) -> Invoker:
    # The PM is stateful: the appliers fold process_state into the session's
    # state first, then this handler reads it. The host returns a full
    # ProcessManagerHandleResponse; the trigger cover is its page context.
    def inv(session, type_url, payload, aux):
        pax = abi_pb2.PmEventAux()
        pax.ParseFromString(aux)
        state = session.ensure_state(key, factory)
        with _handling(PageContext(cover=_cover_of(pax, "trigger_cover"))):
            resp = thunk(
                any_pb2.Any(type_url=type_url, value=payload), state, Destinations(targets)
            )
        if resp is None:
            return None, _STATUS_OK_EMPTY
        return resp.SerializeToString(), _STATUS_OK

    return inv


def _pm_compensation_response(result) -> object | None:
    """The ProcessManagerHandleResponse a PM compensator's result stands for:
    a full response as returned, the ``(process_events, escalation)`` pair
    folded into one, or None for nothing."""
    if result is None or isinstance(result, process_manager_pb2.ProcessManagerHandleResponse):
        return result
    process_events, escalation = result
    resp = process_manager_pb2.ProcessManagerHandleResponse(process_events=process_events)
    if escalation is not None:
        resp.notification.CopyFrom(escalation)
    return resp


def _pm_rejection_invoker(key: int, factory, thunk: PMRejectionThunk) -> Invoker:
    def inv(session, _type_url, _payload, aux):
        rax = abi_pb2.RejectionAux()
        rax.ParseFromString(aux)
        notification = types_pb2.Notification()
        notification.ParseFromString(rax.notification)
        rejection = types_pb2.RejectionNotification()
        rejection.ParseFromString(rax.rejection)
        state = session.ensure_state(key, factory)
        resp = _pm_compensation_response(thunk(notification, rejection, state))
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


def _write_out(out, data: bytes | None) -> None:
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


# What cffi returns if the trampoline itself fails to return a value: an
# INTERNAL status, never STATUS_OK with `out` unset.
_TRAMPOLINE_ERROR = -int(GrpcCode.INTERNAL)


@ffi.callback("angzarr_cb", error=_TRAMPOLINE_ERROR)
def _trampoline(
    host_ctx, callback_id, type_url, type_url_len, payload, payload_len, aux, aux_len, out
):
    """The single C-visible gateway the core calls for every host callback.
    Recovers the dispatch session from host_ctx, routes by callback_id to the
    registered invoker, and writes the response into out. Any raised
    BaseException is caught and surfaced as a coded failure — nothing unwinds
    across the boundary into Rust."""
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
    except BaseException as exc:  # noqa: BLE001 — boundary guard: nothing crosses into Rust
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
        self._next_component = 0
        self._lock = threading.Lock()

    def close(self) -> None:
        """Free the underlying Rust router. Safe to call once."""
        if self._ptr is not None:
            lib.angzarr_router_free(self._ptr)
            self._ptr = None

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_exc) -> None:
        self.close()

    def _component_key(self) -> int:
        """A fresh key identifying one registered component's host state."""
        self._next_component += 1
        return self._next_component

    def _call(self, fn, request: bytes, page: PageContext) -> tuple[int, bytes]:
        """Run one dispatch entry point over ``request`` bytes with a fresh
        session and ``page`` as the dispatch-level page context (callbacks
        whose aux carries a cover narrow it). Returns the dispatch's return
        code and its consumed out bytes."""
        # The session is reached from callbacks via this handle; the core holds
        # it only for the duration of this synchronous call. `handle` must stay
        # referenced until dispatch returns.
        session = _Session(self)
        handle = ffi.new_handle(session)
        out = ffi.new("angzarr_buf*")
        with _handling(page):
            ret = fn(self._ptr, handle, _as_u8(request), len(request), out)
        return ret, _consume_out(out)

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
            key = self._component_key()
            desc = abi_pb2.AggregateDescriptor(name=dispatch.name, domain=dispatch.domain)

            for fq, thunk in dispatch.rebuilder.appliers.items():
                cid = self._assign(_applier_invoker(key, factory, thunk))
                desc.appliers.append(abi_pb2.CallbackEntry(fq_type=fq, callback_id=cid))
            if dispatch.rebuilder.snapshot is not None:
                desc.snapshot_callback_id = self._assign(
                    _snapshot_invoker(key, factory, dispatch.rebuilder.snapshot)
                )
            for fq, thunk in dispatch.commands.items():
                cid = self._assign(_command_invoker(key, factory, thunk))
                desc.commands.append(abi_pb2.CallbackEntry(fq_type=fq, callback_id=cid))
            for compensates, thunks in dispatch.rejections.items():
                entry = abi_pb2.RejectionEntry(compensates=compensates)
                for thunk in thunks:
                    entry.callback_ids.append(self._assign(_rejection_invoker(key, factory, thunk)))
                desc.rejections.append(entry)
            for fq, thunk in dispatch.undoes.items():
                cid = self._assign(_undo_invoker(key, factory, thunk))
                desc.undoes.append(abi_pb2.CallbackEntry(fq_type=fq, callback_id=cid))
            for fq, thunk in dispatch.facts.items():
                cid = self._assign(_fact_invoker(key, factory, thunk))
                desc.facts.append(abi_pb2.CallbackEntry(fq_type=fq, callback_id=cid))
            if isinstance(factory(), Message):
                desc.state_callback_id = self._assign(_state_invoker(key, factory))

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
        ret, resp_bytes = self._call(
            lib.angzarr_router_dispatch,
            contextual_command.SerializeToString(),
            PageContext(cover=_cover_of(contextual_command.command, "cover")),
        )
        if ret == 0:
            resp = command_handler_pb2.BusinessResponse()
            if resp_bytes:
                resp.ParseFromString(resp_bytes)
            return resp
        raise _decode_status(resp_bytes, ret)

    def dispatch_fact(self, fact_request) -> object:
        """Run one FactRequest through the aggregate claiming the facts' cover
        domain: rebuild its state from the prior events, run each fact through
        its fact handler, and return the EventBook of facts to record. Raises a
        CodedError decoded from the core's failure."""
        ret, resp_bytes = self._call(
            lib.angzarr_router_dispatch_fact,
            fact_request.SerializeToString(),
            PageContext(cover=_cover_of(fact_request.facts, "cover")),
        )
        if ret == 0:
            book = types_pb2.EventBook()
            if resp_bytes:
                book.ParseFromString(resp_bytes)
            return book
        raise _decode_status(resp_bytes, ret)

    def dispatch_replay(self, domain: str, replay_request) -> object:
        """Replay a ReplayRequest (a base snapshot, then events) through the
        aggregate registered for ``domain`` (empty selects a sole registered
        aggregate), or the process manager whose own domain it is when no
        aggregate claims it, and return the ReplayResponse carrying its
        rebuilt state, packed. A component whose state is not a protobuf
        message does not support Replay (NO_HANDLER_REGISTERED). Raises a CodedError decoded
        from the core's failure."""
        call = abi_pb2.ReplayCall(domain=domain)
        call.request.CopyFrom(replay_request)
        ret, resp_bytes = self._call(
            lib.angzarr_router_dispatch_replay, call.SerializeToString(), PageContext()
        )
        if ret == 0:
            resp = command_handler_pb2.ReplayResponse()
            if resp_bytes:
                resp.ParseFromString(resp_bytes)
            return resp
        raise _decode_status(resp_bytes, ret)

    def register_projector(self, dispatch: ProjectorDispatch) -> None:
        """Register one projector component: assign callback ids to every
        fold/finish/unknown thunk, serialize the ProjectorDescriptor, and hand
        it to the core with the shared trampoline."""
        with self._lock:
            factory = dispatch.factory
            key = self._component_key()
            desc = abi_pb2.ProjectorDescriptor(name=dispatch.name)
            desc.domains.extend(dispatch.domains)
            for fq, thunk in dispatch.events.items():
                cid = self._assign(_projector_event_invoker(key, factory, thunk))
                desc.events.append(abi_pb2.CallbackEntry(fq_type=fq, callback_id=cid))
            if dispatch.unknown is not None:
                desc.unknown_callback_id = self._assign(
                    _projector_unknown_invoker(dispatch.unknown)
                )
            if dispatch.finisher is not None:
                desc.finish_callback_id = self._assign(
                    _projector_finish_invoker(key, factory, dispatch.finisher)
                )

            desc_bytes = desc.SerializeToString()
            ret = lib.angzarr_router_register_projector(
                self._ptr, _as_u8(desc_bytes), len(desc_bytes), _trampoline
            )
            if ret != 0:
                raise _decode_status(None, ret)

    def dispatch_projector(self, event_book) -> object:
        """Fold one EventBook through the registered projector and return the
        Projection, or raise a CodedError decoded from the core's failure."""
        ret, resp_bytes = self._call(
            lib.angzarr_router_dispatch_projector,
            event_book.SerializeToString(),
            PageContext(cover=_cover_of(event_book, "cover")),
        )
        if ret == 0:
            proj = types_pb2.Projection()
            if resp_bytes:
                proj.ParseFromString(resp_bytes)
            return proj
        raise _decode_status(resp_bytes, ret)

    def register_saga(self, dispatch: SagaDispatch) -> None:
        """Register one saga component: assign callback ids to every event
        thunk, serialize the SagaDescriptor, and hand it to the core with the
        shared trampoline."""
        with self._lock:
            targets = list(dispatch.targets)
            desc = abi_pb2.SagaDescriptor(name=dispatch.name, input_domain=dispatch.input_domain)
            desc.target_domains.extend(targets)
            for fq, thunk in dispatch.events.items():
                cid = self._assign(_saga_event_invoker(targets, thunk))
                desc.events.append(abi_pb2.CallbackEntry(fq_type=fq, callback_id=cid))

            desc_bytes = desc.SerializeToString()
            ret = lib.angzarr_router_register_saga(
                self._ptr, _as_u8(desc_bytes), len(desc_bytes), _trampoline
            )
            if ret != 0:
                raise _decode_status(None, ret)

    def dispatch_saga(self, saga_request) -> object:
        """Run one SagaHandleRequest through the registered saga and return the
        SagaResponse, or raise a CodedError decoded from the core's failure."""
        ret, resp_bytes = self._call(
            lib.angzarr_router_dispatch_saga,
            saga_request.SerializeToString(),
            PageContext(cover=_cover_of(saga_request.source, "cover")),
        )
        if ret == 0:
            resp = saga_pb2.SagaResponse()
            if resp_bytes:
                resp.ParseFromString(resp_bytes)
            return resp
        raise _decode_status(resp_bytes, ret)

    def register_process_manager(self, dispatch: ProcessManagerDispatch) -> None:
        """Register one process-manager component: assign callback ids to every
        applier/snapshot/event/rejection thunk (plus a state packer for Replay
        when the state is a protobuf message), serialize the
        ProcessManagerDescriptor, and hand it to the core with the shared
        trampoline."""
        with self._lock:
            factory = dispatch.rebuilder.factory
            key = self._component_key()
            targets = list(dispatch.targets)
            desc = abi_pb2.ProcessManagerDescriptor(
                name=dispatch.name, pm_domain=dispatch.pm_domain
            )
            desc.target_domains.extend(targets)
            for fq, thunk in dispatch.rebuilder.appliers.items():
                cid = self._assign(_applier_invoker(key, factory, thunk))
                desc.appliers.append(abi_pb2.CallbackEntry(fq_type=fq, callback_id=cid))
            if dispatch.rebuilder.snapshot is not None:
                desc.snapshot_callback_id = self._assign(
                    _snapshot_invoker(key, factory, dispatch.rebuilder.snapshot)
                )
            for input_domain, by_type in dispatch.handlers.items():
                for fq, thunk in by_type.items():
                    cid = self._assign(_pm_event_invoker(key, factory, targets, thunk))
                    desc.events.append(
                        abi_pb2.PmEventEntry(input_domain=input_domain, fq_type=fq, callback_id=cid)
                    )
            for compensates, thunks in dispatch.rejections.items():
                entry = abi_pb2.RejectionEntry(compensates=compensates)
                for thunk in thunks:
                    cid = self._assign(_pm_rejection_invoker(key, factory, thunk))
                    entry.callback_ids.append(cid)
                desc.rejections.append(entry)
            if isinstance(factory(), Message):
                desc.state_callback_id = self._assign(_state_invoker(key, factory))

            desc_bytes = desc.SerializeToString()
            ret = lib.angzarr_router_register_process_manager(
                self._ptr, _as_u8(desc_bytes), len(desc_bytes), _trampoline
            )
            if ret != 0:
                raise _decode_status(None, ret)

    def dispatch_process_manager(self, pm_request) -> object:
        """Run one ProcessManagerHandleRequest through the registered PM and
        return the ProcessManagerHandleResponse, or raise a CodedError decoded
        from the core's failure."""
        ret, resp_bytes = self._call(
            lib.angzarr_router_dispatch_process_manager,
            pm_request.SerializeToString(),
            PageContext(cover=_cover_of(pm_request.trigger, "cover")),
        )
        if ret == 0:
            resp = process_manager_pb2.ProcessManagerHandleResponse()
            if resp_bytes:
                resp.ParseFromString(resp_bytes)
            return resp
        raise _decode_status(resp_bytes, ret)


def abi_version() -> int:
    """The ABI version the linked router-ffi exposes. Bindings check it so a
    binding and a router-ffi artifact that have drifted refuse each other."""
    return int(lib.angzarr_abi_version())
