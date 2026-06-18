"""The CounterAggregate conformance fixture (FIXTURE.md) in Python — the same
behavior the Rust core and Go binding implement. ``observed``, when supplied,
records the CommandContext and rebuilt count each command handler saw (the
historical-state evidence the suite asserts, since state never crosses the
boundary)."""

from __future__ import annotations

from dataclasses import dataclass

from google.protobuf import any_pb2

from .. import (
    AggregateDispatch,
    CommandContext,
    Destinations,
    ProjectorDispatch,
    Rebuilder,
    SagaDispatch,
    reject,
)
from ..gen.io.angzarr.v1 import command_handler_pb2, types_pb2
from ..gen.test.counter import counter_pb2
from .builders import (
    FQ_FAIL_HARD,
    FQ_INCREASE_BY,
    FQ_INCREASED,
    FQ_RESERVE,
    type_url,
)


@dataclass
class Observation:
    """One record of what a command handler saw: the context and rebuilt
    state count."""

    cctx: CommandContext
    count: int


class _Counter:
    """The host state — it never crosses the FFI."""

    __slots__ = ("count",)

    def __init__(self):
        self.count = 0


def _increased_any() -> any_pb2.Any:
    """A single Increased event payload, Any-wrapped with the framework's
    bare-"/" type URL (not the type.googleapis.com Any default)."""
    return any_pb2.Any(
        type_url=type_url(FQ_INCREASED), value=counter_pb2.Increased().SerializeToString()
    )


def _marker_response(name: str):
    """A compensation response carrying one marker event whose type name
    records which compensator ran (the suite asserts on the type URL)."""
    resp = command_handler_pb2.BusinessResponse()
    page = resp.events.pages.add()
    page.event.type_url = type_url("test.counter." + name)
    return resp


def counter_aggregate(observed: list[Observation] | None = None) -> AggregateDispatch:
    def apply_increased(state: _Counter, payload: any_pb2.Any) -> None:
        ev = counter_pb2.Increased()
        ev.ParseFromString(payload.value)
        state.count += 1

    def load_snapshot(state: _Counter, payload: any_pb2.Any) -> None:
        snap = counter_pb2.CounterState()
        snap.ParseFromString(payload.value)
        state.count = snap.count

    rebuilder = (
        Rebuilder(_Counter).apply(FQ_INCREASED, apply_increased).with_snapshot(load_snapshot)
    )

    def on_increase(cmd: any_pb2.Any, state: _Counter, cctx: CommandContext):
        if observed is not None:
            observed.append(Observation(cctx=cctx, count=state.count))
        c = counter_pb2.IncreaseBy()
        c.ParseFromString(cmd.value)
        if c.n == 0:
            raise reject("VALUE_NOT_POSITIVE", "increase amount must be positive")
        book = types_pb2.EventBook()
        for _ in range(c.n):
            book.pages.add().event.CopyFrom(_increased_any())
        return book

    def on_fail_hard(_cmd, _state, _cctx):
        raise RuntimeError("hard failure")

    def on_reserve_first(_n, _rej, _state, _cctx):
        return _marker_response("CompensatedFirst")

    def on_reserve_second(_n, _rej, _state, _cctx):
        return _marker_response("CompensatedSecond")

    return (
        AggregateDispatch("counter-aggregate", "counter", rebuilder)
        .on_command(FQ_INCREASE_BY, on_increase)
        .on_command(FQ_FAIL_HARD, on_fail_hard)
        .on_rejected(FQ_RESERVE, on_reserve_first)
        .on_rejected(FQ_RESERVE, on_reserve_second)
    )


class _Projection:
    """The host projection state — it never crosses the FFI; the harness reads
    the fold count back off the finished Projection."""

    __slots__ = ("count",)

    def __init__(self):
        self.count = 0


def counter_projector() -> ProjectorDispatch:
    """The CounterProjector conformance fixture (projector.feature) in Python:
    over the "counter" domain it folds each Increased into a count, then
    finishes into a Projection carrying that count as its sequence."""

    def fold_increased(state: _Projection, _event: any_pb2.Any) -> None:
        state.count += 1

    def finish(state: _Projection, events) -> object:
        proj = types_pb2.Projection()
        if events.HasField("cover"):
            proj.cover.CopyFrom(events.cover)
        proj.projector = "counter-projector"
        proj.sequence = state.count
        return proj

    return (
        ProjectorDispatch("counter-projector", _Projection)
        .for_domains("counter")
        .on_event(FQ_INCREASED, fold_increased)
        .finish(finish)
    )


def order_saga() -> SagaDispatch:
    """The OrderSaga conformance fixture (saga.feature) in Python: it
    translates each Increased source event into one Reserve command for
    "inventory" (stamped from the destination sequence when present), and
    compensates a rejected Reserve by injecting one fact event."""

    def on_increased(_event: any_pb2.Any, dests: Destinations):
        cmd = types_pb2.CommandBook()
        cmd.cover.domain = "inventory"
        cmd.pages.add().command.type_url = type_url(FQ_RESERVE)
        if dests.has("inventory"):
            dests.stamp_command(cmd, "inventory")
        return [cmd], []

    def on_reserve_rejected(_notification, _rejection):
        event = types_pb2.EventBook()
        event.pages.add()
        return [event]

    return (
        SagaDispatch("order-saga", "order", ["inventory"])
        .on_event(FQ_INCREASED, on_increased)
        .on_rejected(FQ_RESERVE, on_reserve_rejected)
    )
