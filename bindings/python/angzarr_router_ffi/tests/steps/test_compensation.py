"""Compensation-routing harness — runs the shared
``conformance/features/compensation.feature`` against payment and inventory
aggregates built through the binding's hand-written aggregate API
(``on_rejected`` with plain and domain-qualified ``compensates`` entries, and
``on_undo``)."""

from __future__ import annotations

import pytest
from pytest_bdd import given, parsers, scenarios, then, when

from ... import AggregateDispatch, CodedError, GrpcCode, Rebuilder, Router, pack
from ...gen.io.angzarr.v1 import command_handler_pb2, types_pb2
from ..builders import FQ_RESERVE, fq_from_url, type_url

scenarios("compensation.feature")


def _one_event(name: str):
    """A business response carrying one header-less event page of
    ``test.counter.<name>``."""
    resp = command_handler_pb2.BusinessResponse()
    resp.events.pages.add().event.type_url = type_url("test.counter." + name)
    return resp


def _emitting(name: str):
    def handler(*_args):
        return _one_event(name)

    return handler


def _payment_aggregate(entries: list[tuple[str, str]]) -> AggregateDispatch:
    """The payment aggregate (domain "payment"): one compensation handler per
    (compensates entry, emitted event name) pair."""
    dispatch = AggregateDispatch("Payment", "payment", Rebuilder(factory=lambda: None))
    for compensates, event in entries:
        dispatch.on_rejected(compensates, _emitting(event))
    return dispatch


def _inventory_aggregate() -> AggregateDispatch:
    """The inventory aggregate (domain "inventory"): undoes AdjustStock with
    StockAdjustmentReverted and Reserve with StockReleased."""
    return (
        AggregateDispatch("Inventory", "inventory", Rebuilder(factory=lambda: None))
        .on_undo("test.counter.AdjustStock", _emitting("StockAdjustmentReverted"))
        .on_undo(FQ_RESERVE, _emitting("StockReleased"))
    )


def _notification_command(domain: str, payload, next_sequence: int | None):
    """A Notification command addressed to ``domain`` wrapping ``payload``,
    over prior history whose next sequence is ``next_sequence`` when given."""
    notification = types_pb2.Notification()
    notification.payload.CopyFrom(payload)
    cc = types_pb2.ContextualCommand()
    cc.command.cover.domain = domain
    cc.command.pages.add().command.CopyFrom(pack(notification))
    if next_sequence is not None:
        cc.events.next_sequence = next_sequence
        page = cc.events.pages.add()
        page.header.sequence = max(next_sequence - 1, 0)
        page.event.type_url = type_url("test.counter.Unrelated")
    return cc


def _rejection_sent_to(command: str, target_domain: str, next_sequence: int | None = None):
    rejection = types_pb2.RejectionNotification()
    rejection.rejected_command.cover.domain = target_domain
    rejection.rejected_command.pages.add().command.type_url = type_url("test.counter." + command)
    return _notification_command("payment", pack(rejection), next_sequence)


def _compensate_for(command: str):
    compensate = types_pb2.Compensate(
        command_type="test.counter." + command, sequences=[0], reason="aborted"
    )
    return _notification_command("inventory", pack(compensate), None)


class _World:
    def __init__(self):
        self.router = Router()
        self.resp = None
        self.err: CodedError | None = None

    def build(self, dispatch: AggregateDispatch) -> None:
        self.router.register_aggregate(dispatch)

    def dispatch(self, cc) -> None:
        try:
            self.resp = self.router.dispatch(cc)
            self.err = None
        except CodedError as exc:
            self.err = exc
            self.resp = None

    def pages(self):
        assert self.err is None, f"dispatch failed: {self.err}"
        return list(self.resp.events.pages)

    def close(self) -> None:
        self.router.close()


@pytest.fixture
def world():
    w = _World()
    yield w
    w.close()


@given(parsers.re(r"a payment aggregate compensating Reserve from any domain with (?P<event>\w+)"))
def _payment_unqualified(world, event):
    world.build(_payment_aggregate([(FQ_RESERVE, event)]))


@given(
    parsers.re(
        r"a second payment aggregate compensating Reserve from any domain with (?P<event>\w+)"
    )
)
def _second_payment(world, event):
    world.build(_payment_aggregate([(FQ_RESERVE, event)]))


@given(
    parsers.re(
        r'a payment aggregate compensating Reserve from "(?P<first_domain>[^"]*)" with '
        r'(?P<first_event>\w+) and from "(?P<second_domain>[^"]*)" with (?P<second_event>\w+)'
    )
)
def _payment_qualified(world, first_domain, first_event, second_domain, second_event):
    world.build(
        _payment_aggregate(
            [
                (f"{first_domain}:{FQ_RESERVE}", first_event),
                (f"{second_domain}:{FQ_RESERVE}", second_event),
            ]
        )
    )


@given(
    "an inventory aggregate undoing AdjustStock with StockAdjustmentReverted "
    "and Reserve with StockReleased"
)
def _inventory(world):
    world.build(_inventory_aggregate())


@when(
    parsers.re(
        r'a rejection of (?P<command>\w+) sent to "(?P<domain>[^"]*)" '
        r"is dispatched to the payment aggregate"
    )
)
def _rejection_sent(world, command, domain):
    world.dispatch(_rejection_sent_to(command, domain))


@when(
    parsers.re(
        r'a rejection of (?P<command>\w+) sent to "(?P<domain>[^"]*)" is dispatched to the '
        r"payment aggregate over history ending at sequence (?P<last>\d+)"
    )
)
def _rejection_over_history(world, command, domain, last):
    world.dispatch(_rejection_sent_to(command, domain, int(last) + 1))


@when(parsers.re(r"a Compensate for (?P<command>\w+) is dispatched to the inventory aggregate"))
def _compensate(world, command):
    world.dispatch(_compensate_for(command))


@then(parsers.re(r"the aggregate emits one (?P<event>\w+) event"))
def _emits_one(world, event):
    pages = world.pages()
    assert len(pages) == 1, f"emitted {len(pages)} events, want exactly one"
    assert fq_from_url(pages[0].event.type_url) == "test.counter." + event


@then(
    parsers.re(
        r"the aggregates emit (?P<first>\w+) at sequence (?P<first_seq>\d+) "
        r"then (?P<second>\w+) at sequence (?P<second_seq>\d+)"
    )
)
def _emit_in_order(world, first, first_seq, second, second_seq):
    got = []
    for page in world.pages():
        assert page.header.WhichOneof("sequence_type") == "sequence"
        got.append((fq_from_url(page.event.type_url), page.header.sequence))
    assert got == [
        ("test.counter." + first, int(first_seq)),
        ("test.counter." + second, int(second_seq)),
    ]


@then("the aggregate emits nothing")
def _emits_nothing(world):
    assert world.pages() == []


@then(parsers.re(r"the emitted event takes sequence (?P<seq>\d+)"))
def _takes_sequence(world, seq):
    pages = world.pages()
    assert pages, "no event was emitted"
    assert pages[0].header.WhichOneof("sequence_type") == "sequence"
    assert pages[0].header.sequence == int(seq)


@then(parsers.re(r"the dispatch fails with (?P<code>[A-Z_]+) as UNIMPLEMENTED"))
def _fails_unimplemented(world, code):
    assert world.err is not None, f"expected failure {code}, got a success"
    assert world.err.code == code
    assert world.err.grpc == GrpcCode.UNIMPLEMENTED
