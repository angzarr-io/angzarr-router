"""Dispatch through the hand-written registration APIs (no generated wiring):
ordered compensator fan-out across the FFI, the trampoline's boundary guard
against exceptions that are not ``Exception`` subclasses, declared-output
Destinations for sagas and process managers, and process-manager compensators
returning a full response."""

from __future__ import annotations

import pytest

from .. import (
    AggregateDispatch,
    CodedError,
    Destinations,
    GrpcCode,
    ProcessManagerDispatch,
    Rebuilder,
    Router,
    SagaDispatch,
)
from ..gen.io.angzarr.v1 import command_handler_pb2, process_manager_pb2, saga_pb2, types_pb2
from ..gen.test.counter import counter_pb2
from . import builders


class _HandlerAbort(BaseException):
    """A BaseException that is not an Exception, raised by a handler."""


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


def test_handler_base_exception_surfaces_as_internal_failure():
    """A handler raising a BaseException that is not an Exception fails the
    dispatch as an unclassified INTERNAL error rather than passing as
    success."""

    def abort(cmd, state, cctx):
        raise _HandlerAbort("handler aborted")

    dispatch = _counter_dispatch().on_command(builders.FQ_INCREASE_BY, abort)
    with Router() as router:
        router.register_aggregate(dispatch)
        with pytest.raises(CodedError) as exc:
            router.dispatch(builders.increase_command(1))

    assert exc.value.code == "UNHANDLED_HANDLER_ERROR"
    assert exc.value.grpc == GrpcCode.INTERNAL
    assert "handler aborted" in exc.value.message


def test_destinations_are_the_declared_output_domains():
    dests = Destinations(["inventory", "billing"])
    assert dests.has("inventory")
    assert dests.has("billing")
    assert not dests.has("shipping")
    assert dests.domains() == ["inventory", "billing"]
    assert Destinations().domains() == []
    assert not hasattr(dests, "stamp_command")
    assert not hasattr(dests, "sequence_for")


def _saga_source(seq: int):
    req = saga_pb2.SagaHandleRequest()
    req.source.cover.domain = "order"
    page = req.source.pages.add()
    page.event.type_url = builders.type_url(builders.FQ_INCREASED)
    page.header.sequence = seq
    return req


def _reserve_to(domain: str):
    cmd = types_pb2.CommandBook()
    cmd.cover.domain = domain
    cmd.pages.add().command.type_url = builders.type_url(builders.FQ_RESERVE)
    return cmd


def test_saga_destinations_are_its_registered_targets_and_commands_are_deferred():
    seen: list[list[str]] = []

    def translate(event, dests, source_cover):
        seen.append(dests.domains())
        return [_reserve_to("inventory"), _reserve_to("billing")], []

    saga = SagaDispatch("order-saga", "order", targets=["inventory", "billing"])
    saga.on_event(builders.FQ_INCREASED, translate)
    with Router() as router:
        router.register_saga(saga)
        resp = router.dispatch_saga(_saga_source(7))

    assert seen == [["inventory", "billing"]]
    assert [c.cover.domain for c in resp.commands] == ["inventory", "billing"]
    builders.assert_deferred(resp.commands[0], "order", 7, 0)
    builders.assert_deferred(resp.commands[1], "order", 7, 1)


def test_saga_registration_has_no_rejection_api():
    assert not hasattr(SagaDispatch("s", "order"), "on_rejected")


def _pm_trigger(seq: int):
    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.domain = "counter"
    page = req.trigger.pages.add()
    page.event.type_url = builders.type_url(builders.FQ_INCREASED)
    page.header.sequence = seq
    return req


def test_process_manager_destinations_are_its_registered_targets():
    seen: list[list[str]] = []

    def react(event, state, dests):
        seen.append(dests.domains())
        resp = process_manager_pb2.ProcessManagerHandleResponse()
        resp.commands.append(_reserve_to("inventory"))
        return resp

    pm = ProcessManagerDispatch(
        "Reserving", "reserving-pm", Rebuilder(factory=counter_pb2.CounterState), ["inventory"]
    )
    pm.on_event("counter", builders.FQ_INCREASED, react)
    with Router() as router:
        router.register_process_manager(pm)
        resp = router.dispatch_process_manager(_pm_trigger(3))

    assert seen == [["inventory"]]
    builders.assert_deferred(resp.commands[0], "counter", 3, 0)


def test_process_manager_targets_default_to_none_declared():
    seen: list[list[str]] = []

    def react(event, state, dests):
        seen.append(dests.domains())
        return process_manager_pb2.ProcessManagerHandleResponse()

    pm = ProcessManagerDispatch("Quiet", "quiet-pm", Rebuilder(factory=counter_pb2.CounterState))
    pm.on_event("counter", builders.FQ_INCREASED, react)
    with Router() as router:
        router.register_process_manager(pm)
        router.dispatch_process_manager(_pm_trigger(0))

    assert seen == [[]]


def _pm_rejection(seq: int):
    rejection = types_pb2.RejectionNotification()
    rejection.rejected_command.CopyFrom(_reserve_to("inventory"))
    notification = types_pb2.Notification()
    notification.payload.type_url = builders.type_url("io.angzarr.v1.RejectionNotification")
    notification.payload.value = rejection.SerializeToString()
    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.domain = "reserving-pm"
    page = req.trigger.pages.add()
    page.header.sequence = seq
    page.event.type_url = builders.type_url("io.angzarr.v1.Notification")
    page.event.value = notification.SerializeToString()
    return req


def test_process_manager_compensator_full_response_keeps_commands_deferred():
    def compensate(notification, rejection, state):
        resp = process_manager_pb2.ProcessManagerHandleResponse()
        release = resp.commands.add()
        release.cover.domain = "inventory"
        release.pages.add().command.type_url = builders.type_url("test.counter.Release")
        resp.process_events.add().cover.domain = "reserving-pm"
        resp.notification.cover.domain = "escalated"
        return resp

    pm = ProcessManagerDispatch(
        "Reserving", "reserving-pm", Rebuilder(factory=counter_pb2.CounterState), ["inventory"]
    )
    pm.on_rejected(builders.FQ_RESERVE, compensate)
    with Router() as router:
        router.register_process_manager(pm)
        resp = router.dispatch_process_manager(_pm_rejection(5))

    assert len(resp.commands) == 1
    assert builders.fq_from_url(resp.commands[0].pages[0].command.type_url) == (
        "test.counter.Release"
    )
    builders.assert_deferred(resp.commands[0], "reserving-pm", 5, 0)
    assert [b.cover.domain for b in resp.process_events] == ["reserving-pm"]
    assert resp.notification.cover.domain == "escalated"


def test_process_manager_compensator_tuple_shape_still_supported():
    def compensate(notification, rejection, state):
        book = types_pb2.EventBook()
        book.cover.domain = "reserving-pm"
        return [book], None

    pm = ProcessManagerDispatch(
        "Reserving", "reserving-pm", Rebuilder(factory=counter_pb2.CounterState)
    )
    pm.on_rejected(builders.FQ_RESERVE, compensate)
    with Router() as router:
        router.register_process_manager(pm)
        resp = router.dispatch_process_manager(_pm_rejection(5))

    assert [b.cover.domain for b in resp.process_events] == ["reserving-pm"]
    assert len(resp.commands) == 0
    assert not resp.HasField("notification")
