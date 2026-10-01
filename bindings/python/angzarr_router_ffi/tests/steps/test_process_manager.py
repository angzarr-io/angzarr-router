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
from ...gen.test.counter import audit_process_manager_angzarr, order_process_manager_angzarr
from ..builders import FQ_INCREASED, FQ_RESERVE, assert_deferred, type_url
from ..fixture import AUDIT_MARK, AuditProcessManager, OrderProcessManager

scenarios("process_manager.feature")


class _World:
    """One scenario's state: a router the Given step registers the PM
    fixtures on, and the dispatch outcome."""

    def __init__(self):
        self.router = Router()
        self.resp = None
        self.err: CodedError | None = None

    def facts_marked(self, audit: bool) -> int:
        """How many response facts the audit PM (``audit``) or the order PM
        (not ``audit``) emitted, told apart by the audit cover mark."""
        assert self.err is None, f"dispatch failed: {self.err}"
        return sum(1 for f in self.resp.facts if (f.cover.domain == AUDIT_MARK) == audit)

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


def _trigger(domain: str, fqs: list[str], state=None, newest_seq: int | None = None):
    """A trigger of ``fqs`` pages in ``domain``, the newest at explicit
    sequence ``newest_seq`` when given, over the PM's prior ``state``."""
    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.domain = domain
    for fq in fqs:
        req.trigger.pages.add().event.type_url = type_url(fq)
    if newest_seq is not None:
        req.trigger.pages[-1].header.sequence = newest_seq
    if state is not None:
        req.process_state.CopyFrom(state)
    return req


def _compensate(fq_command: str):
    """A trigger in the order PM's own domain whose page is a Notification
    carrying a Compensate for an executed ``fq_command``."""
    compensate = types_pb2.Compensate(command_type=fq_command, sequences=[0], reason="aborted")
    notification = types_pb2.Notification()
    notification.payload.type_url = type_url("io.angzarr.v1.Compensate")
    notification.payload.value = compensate.SerializeToString()
    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.domain = "order-pm"
    page = req.trigger.pages.add()
    page.event.type_url = type_url("io.angzarr.v1.Notification")
    page.event.value = notification.SerializeToString()
    return req


def _state_of(n: int, owner: str | None = None):
    """A process-state book of n Increased events; ``owner`` sets its cover
    domain (the PM the state belongs to)."""
    book = types_pb2.EventBook()
    if owner is not None:
        book.cover.domain = owner
    for _ in range(n):
        book.pages.add().event.type_url = type_url(FQ_INCREASED)
    return book


def _rejection(fq_command: str, issuer: str | None = None):
    """A rejection of fq_command delivered as a Notification trigger. With an
    ``issuer`` the trigger cover is the issuer's domain and the rejected
    command's angzarr_deferred header names the issuer as its source."""
    rejection = types_pb2.RejectionNotification()
    rejection.rejected_command.cover.domain = "inventory"
    page = rejection.rejected_command.pages.add()
    page.command.type_url = type_url(fq_command)
    if issuer is not None:
        page.header.angzarr_deferred.source.domain = issuer
    notification = types_pb2.Notification()
    notification.payload.type_url = type_url("io.angzarr.v1.RejectionNotification")
    notification.payload.value = rejection.SerializeToString()

    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.domain = issuer if issuer is not None else "counter"
    trigger_page = req.trigger.pages.add()
    trigger_page.event.type_url = type_url("io.angzarr.v1.Notification")
    trigger_page.event.value = notification.SerializeToString()
    return req


@given("an order process-manager")
def _an_order_pm(world):
    order_process_manager_angzarr.register_order_process_manager(
        world.router, OrderProcessManager()
    )


@given("co-resident order and audit process-managers")
def _co_resident_pms(world):
    order_process_manager_angzarr.register_order_process_manager(
        world.router, OrderProcessManager()
    )
    audit_process_manager_angzarr.register_audit_process_manager(
        world.router, AuditProcessManager()
    )


@when(
    parsers.re(
        r'an Increased trigger in domain "(?P<domain>[^"]*)" at sequence (?P<seq>\d+) is dispatched'
    )
)
def _increased_at(world, domain, seq):
    world.dispatch(_trigger(domain, [FQ_INCREASED], newest_seq=int(seq)))


@when("a Compensate for Reserve is dispatched to the order process-manager")
def _compensate_for_reserve(world):
    world.dispatch(_compensate(FQ_RESERVE))


@when(parsers.re(r'an Increased trigger in domain "(?P<domain>[^"]*)" is dispatched$'))
def _increased_in_domain(world, domain):
    world.dispatch(_trigger(domain, [FQ_INCREASED]))


@when("a trigger whose newest page is an undeclared event is dispatched")
def _newest_undeclared(world):
    world.dispatch(_trigger("counter", [FQ_INCREASED, "test.counter.Unwatched"]))


@when(parsers.re(r"an Increased trigger is dispatched over a prior state of (?P<n>\d+) events"))
def _increased_over_state(world, n):
    world.dispatch(_trigger("counter", [FQ_INCREASED], state=_state_of(int(n))))


@when(
    parsers.re(
        r'an Increased trigger is dispatched over a prior "(?P<owner>[^"]*)" state of (?P<n>\d+) events'
    )
)
def _increased_over_owned_state(world, owner, n):
    world.dispatch(_trigger("counter", [FQ_INCREASED], state=_state_of(int(n), owner)))


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


@when(parsers.re(r'a rejection of Reserve issued by "(?P<issuer>[^"]*)" is dispatched'))
def _rejection_reserve_issued_by(world, issuer):
    world.dispatch(_rejection(FQ_RESERVE, issuer))


@then(parsers.re(r'the process-manager emits one command to "(?P<target>[^"]*)"'))
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
    assert_deferred(world.resp.commands[0], "counter", int(seq), int(index))


@then("the process-manager emits no commands")
def _emits_no_commands(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.commands) == 0


@then(parsers.re(r"the process-manager rebuilt (?P<n>\d+) prior state events"))
def _rebuilt_n(world, n):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.facts) == int(n)


@then(parsers.re(r"the order process-manager rebuilt (?P<n>\d+) prior state events"))
def _order_rebuilt_n(world, n):
    assert world.facts_marked(audit=False) == int(n)


@then(parsers.re(r"the audit process-manager rebuilt (?P<n>\d+) prior state events"))
def _audit_rebuilt_n(world, n):
    assert world.facts_marked(audit=True) == int(n)


@then("the order process-manager did not react")
def _order_did_not_react(world):
    assert world.facts_marked(audit=False) == 0
    assert len(world.resp.commands) == 0


@then("only the audit process-manager compensates")
def _only_audit_compensates(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.process_events) == 1
    assert world.resp.process_events[0].cover.domain == AUDIT_MARK
    assert not world.resp.HasField("notification")


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
