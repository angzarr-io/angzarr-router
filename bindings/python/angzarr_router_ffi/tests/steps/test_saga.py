"""Saga conformance harness — runs the shared
``conformance/features/saga.feature`` (the same one the Rust cucumber-rs and Go
godog harnesses run) against the Python binding via pytest-bdd. Only the step
layer is new; the behavior spec is shared, unchanged."""

from __future__ import annotations

import pytest
from pytest_bdd import given, parsers, scenarios, then, when

from ... import CodedError, Router
from ...gen.io.angzarr.v1 import saga_pb2, types_pb2
from ...gen.test.counter import order_saga_angzarr
from ..builders import FQ_INCREASED, FQ_RESERVE, assert_deferred, type_url
from ..fixture import OrderSaga

scenarios("saga.feature")


class _World:
    """One scenario's state: a router with the saga fixture registered, and
    the dispatch outcome."""

    def __init__(self):
        self.router = Router()
        order_saga_angzarr.register_order_saga(self.router, OrderSaga())
        self.resp = None
        self.err: CodedError | None = None

    def dispatch(self, request) -> None:
        try:
            self.resp = self.router.dispatch_saga(request)
            self.err = None
        except CodedError as exc:
            self.err = exc
            self.resp = None

    def close(self) -> None:
        self.router.close()


@pytest.fixture
def world():
    w = _World()
    yield w
    w.close()


def _event_source(fq: str, seq: int | None = None):
    """A SagaHandleRequest carrying one event of fq in the "order" domain, at
    explicit sequence ``seq`` when given."""
    req = saga_pb2.SagaHandleRequest()
    req.source.cover.domain = "order"
    page = req.source.pages.add()
    page.event.type_url = type_url(fq)
    if seq is not None:
        page.header.sequence = seq
    return req


def _rejection_source(fq_command: str):
    """A SagaHandleRequest whose source is a rejection Notification for
    fq_command (sagas receive no rejections, so it emits nothing)."""
    rejection = types_pb2.RejectionNotification()
    rejection.rejected_command.cover.domain = "inventory"
    rejection.rejected_command.pages.add().command.type_url = type_url(fq_command)

    notification = types_pb2.Notification()
    notification.payload.type_url = type_url("io.angzarr.v1.RejectionNotification")
    notification.payload.value = rejection.SerializeToString()

    req = saga_pb2.SagaHandleRequest()
    req.source.cover.domain = "order"
    page = req.source.pages.add()
    page.event.type_url = type_url("io.angzarr.v1.Notification")
    page.event.value = notification.SerializeToString()
    return req


@given(parsers.re(r'an order saga delivering to "(?P<target>[^"]*)"'))
def _an_order_saga(world, target):
    # Each scenario's fresh saga is registered in _World.__init__.
    pass


@when(parsers.re(r"an Increased event at sequence (?P<seq>\d+) is dispatched"))
def _increased_at(world, seq):
    world.dispatch(_event_source(FQ_INCREASED, int(seq)))


@when("a Reserve event is dispatched")
def _reserve_event(world):
    world.dispatch(_event_source(FQ_RESERVE))


@when("a source with no pages is dispatched")
def _empty_source(world):
    req = saga_pb2.SagaHandleRequest()
    req.source.SetInParent()
    world.dispatch(req)


@when("a request with no source is dispatched")
def _missing_source(world):
    world.dispatch(saga_pb2.SagaHandleRequest())


@when("a rejection of Reserve is dispatched")
def _rejection_reserve(world):
    world.dispatch(_rejection_source(FQ_RESERVE))


@then(parsers.re(r'the saga emits one command to "(?P<target>[^"]*)"'))
def _emits_one_command(world, target):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.commands) == 1
    assert world.resp.commands[0].cover.domain == target


@then(
    parsers.re(
        r"the command is deferred from source sequence (?P<seq>\d+) at command index (?P<index>\d+)"
    )
)
def _command_is_deferred(world, seq, index):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert_deferred(world.resp.commands[0], "order", int(seq), int(index))


@then("the saga emits no commands")
def _emits_no_commands(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.commands) == 0


@then("the saga injects no events")
def _injects_no_events(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.events) == 0


@then(parsers.re(r"the dispatch fails with (?P<code>[A-Z_]+)"))
def _fails_with(world, code):
    assert world.err is not None, f"expected failure {code}, got a success"
    assert world.err.code == code
