"""Dispatch through the hand-written aggregate registration API (no generated
wiring): ordered compensator fan-out across the FFI."""

from __future__ import annotations

from .. import AggregateDispatch, Rebuilder, Router
from ..gen.io.angzarr.v1 import command_handler_pb2
from ..gen.test.counter import counter_pb2
from . import builders


def _counter_dispatch() -> AggregateDispatch:
    return AggregateDispatch(
        name="CounterAggregate",
        domain="counter",
        rebuilder=Rebuilder(factory=counter_pb2.CounterState),
    )


def _compensator(marker: str, calls: list[str]):
    def compensate(notification, rejection, state, cctx):
        calls.append(marker)
        resp = command_handler_pb2.BusinessResponse()
        resp.events.pages.add().event.type_url = builders.type_url(marker)
        return resp

    return compensate


def test_two_compensators_fan_out_in_registration_order():
    """Two compensators registered for one rejected command type both run, in
    registration order, and their events merge in that order."""
    calls: list[str] = []
    dispatch = _counter_dispatch()
    dispatch.on_rejected(builders.FQ_RESERVE, _compensator("test.counter.CompensatedFirst", calls))
    dispatch.on_rejected(builders.FQ_RESERVE, _compensator("test.counter.CompensatedSecond", calls))
    with Router() as router:
        router.register_aggregate(dispatch)
        resp = router.dispatch(builders.rejection_command(builders.FQ_RESERVE))

    assert calls == ["test.counter.CompensatedFirst", "test.counter.CompensatedSecond"]
    pages = resp.events.pages
    assert len(pages) == 2
    assert [builders.fq_from_url(p.event.type_url) for p in pages] == [
        "test.counter.CompensatedFirst",
        "test.counter.CompensatedSecond",
    ]
