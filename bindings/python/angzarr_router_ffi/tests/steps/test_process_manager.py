"""Process-manager conformance harness — runs the shared
``conformance/features/process_manager.feature`` (the same one the Rust
cucumber-rs and Go godog harnesses run) against the Python binding via
pytest-bdd. Only the step layer is new; the behavior spec is shared,
unchanged."""

from __future__ import annotations

import pytest
from pytest_bdd import given, parsers, scenarios, then, when

from ... import CodedError, Router
from ...gen.io.angzarr.v1 import process_manager_pb2, types_pb2
from ..builders import FQ_INCREASED, FQ_RESERVE, type_url
from ..fixture import order_pm

scenarios("process_manager.feature")


class _World:
    """One scenario's state: a router with the PM fixture registered, and the
    dispatch outcome."""

    def __init__(self):
        self.router = Router()
        self.router.register_process_manager(order_pm())
        self.resp = None
        self.err: CodedError | None = None

    def dispatch(self, request) -> None:
        try:
            self.resp = self.router.dispatch_process_manager(request)
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


def _trigger(domain: str, fqs: list[str], state=None, dest: dict[str, int] | None = None):
    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.domain = domain
    for fq in fqs:
        req.trigger.pages.add().event.type_url = type_url(fq)
    if state is not None:
        req.process_state.CopyFrom(state)
    for d, seq in (dest or {}).items():
        req.destination_sequences[d] = seq
    return req


def _state_of(n: int):
    book = types_pb2.EventBook()
    for _ in range(n):
        book.pages.add().event.type_url = type_url(FQ_INCREASED)
    return book


def _rejection(fq_command: str):
    rejection = types_pb2.RejectionNotification()
    rejection.rejected_command.cover.domain = "inventory"
    rejection.rejected_command.pages.add().command.type_url = type_url(fq_command)
    notification = types_pb2.Notification()
    notification.payload.type_url = type_url("io.angzarr.v1.RejectionNotification")
    notification.payload.value = rejection.SerializeToString()

    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.domain = "counter"
    page = req.trigger.pages.add()
    page.event.type_url = type_url("io.angzarr.v1.Notification")
    page.event.value = notification.SerializeToString()
    return req


@given("an order process-manager")
def _an_order_pm(world):
    pass


@when(
    parsers.re(
        r'an Increased trigger in domain "(?P<domain>[^"]*)" is dispatched with destination inventory sequence (?P<seq>\d+)'
    )
)
def _increased_with_destination(world, domain, seq):
    world.dispatch(_trigger(domain, [FQ_INCREASED], dest={"inventory": int(seq)}))


@when(parsers.re(r'an Increased trigger in domain "(?P<domain>[^"]*)" is dispatched$'))
def _increased_in_domain(world, domain):
    world.dispatch(_trigger(domain, [FQ_INCREASED]))


@when("a trigger whose newest page is an undeclared event is dispatched")
def _newest_undeclared(world):
    world.dispatch(_trigger("counter", [FQ_INCREASED, "test.counter.Unwatched"]))


@when(parsers.re(r"an Increased trigger is dispatched over a prior state of (?P<n>\d+) events"))
def _increased_over_state(world, n):
    world.dispatch(_trigger("counter", [FQ_INCREASED], state=_state_of(int(n))))


@when("a request with no trigger is dispatched")
def _no_trigger(world):
    world.dispatch(process_manager_pb2.ProcessManagerHandleRequest())


@when("a trigger with no pages is dispatched")
def _empty_trigger(world):
    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.SetInParent()
    world.dispatch(req)


@when("a rejection of Reserve is dispatched")
def _rejection_reserve(world):
    world.dispatch(_rejection(FQ_RESERVE))


@then(parsers.re(r'the process-manager emits one command to "(?P<target>[^"]*)"'))
def _emits_one_command(world, target):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.commands) == 1
    assert world.resp.commands[0].cover.domain == target


@then(parsers.re(r"the command carries destination sequence (?P<seq>\d+)"))
def _command_carries_sequence(world, seq):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert world.resp.commands[0].pages[0].header.sequence == int(seq)


@then("the process-manager emits no commands")
def _emits_no_commands(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.commands) == 0


@then(parsers.re(r"the process-manager rebuilt (?P<n>\d+) prior state events"))
def _rebuilt_n(world, n):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.facts) == int(n)


@then("the process-manager emits one process event")
def _emits_one_process_event(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.process_events) == 1


@then("the process-manager escalates")
def _escalates(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert world.resp.HasField("notification")


@then(parsers.re(r"the dispatch fails with (?P<code>[A-Z_]+)"))
def _fails_with(world, code):
    assert world.err is not None, f"expected failure {code}, got a success"
    assert world.err.code == code
