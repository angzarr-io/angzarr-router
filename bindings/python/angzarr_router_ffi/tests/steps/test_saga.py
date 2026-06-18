"""Saga conformance harness — runs the shared
``conformance/features/saga.feature`` (the same one the Rust cucumber-rs and Go
godog harnesses run) against the Python binding via pytest-bdd. Only the step
layer is new; the behavior spec is shared, unchanged."""

from __future__ import annotations

import pytest
from pytest_bdd import given, parsers, scenarios, then, when

from ... import CodedError, Router
from ...gen.io.angzarr.v1 import saga_pb2, types_pb2
from ..builders import FQ_INCREASED, FQ_RESERVE, type_url
from ..fixture import order_saga

scenarios("saga.feature")


class _World:
    """One scenario's state: a router with the saga fixture registered, and
    the dispatch outcome."""

    def __init__(self):
        self.router = Router()
        self.router.register_saga(order_saga())
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


def _event_source(fq: str, dest: dict[str, int] | None):
    """A SagaHandleRequest carrying one event of fq in the "order" domain plus
    the destination-sequence map."""
    req = saga_pb2.SagaHandleRequest()
    req.source.cover.domain = "order"
    req.source.pages.add().event.type_url = type_url(fq)
    for domain, seq in (dest or {}).items():
        req.destination_sequences[domain] = seq
    return req


def _rejection_source(fq_command: str):
    """A SagaHandleRequest whose source is a rejection Notification for
    fq_command — routes to the compensation path."""
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


@when(
    parsers.re(r"an Increased event is dispatched with destination inventory sequence (?P<seq>\d+)")
)
def _increased_with_destination(world, seq):
    world.dispatch(_event_source(FQ_INCREASED, {"inventory": int(seq)}))


@when("a Reserve event is dispatched")
def _reserve_event(world):
    world.dispatch(_event_source(FQ_RESERVE, None))


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


@when("a rejection of Unwatched is dispatched")
def _rejection_unwatched(world):
    world.dispatch(_rejection_source("test.counter.Unwatched"))


@then(parsers.re(r'the saga emits one command to "(?P<target>[^"]*)"'))
def _emits_one_command(world, target):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.commands) == 1
    assert world.resp.commands[0].cover.domain == target


@then(parsers.re(r"the command carries destination sequence (?P<seq>\d+)"))
def _command_carries_sequence(world, seq):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert world.resp.commands[0].pages[0].header.sequence == int(seq)


@then("the saga emits no commands")
def _emits_no_commands(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.commands) == 0


@then("the saga injects one fact event")
def _injects_one_event(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.events) == 1


@then("the saga injects no events")
def _injects_no_events(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.events) == 0


@then(parsers.re(r"the dispatch fails with (?P<code>[A-Z_]+)"))
def _fails_with(world, code):
    assert world.err is not None, f"expected failure {code}, got a success"
    assert world.err.code == code
