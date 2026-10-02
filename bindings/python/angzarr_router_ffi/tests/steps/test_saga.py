"""Saga conformance harness — runs the shared
``conformance/features/saga.feature`` (the same one the Rust cucumber-rs and Go
godog harnesses run) against the Python binding via pytest-bdd. Only the step
layer is new; the behavior spec is shared, unchanged."""

from __future__ import annotations

import hashlib

import pytest
from pytest_bdd import given, parsers, scenarios, then, when

from ... import CodedError, GrpcCode, Router, SagaDispatch
from ...gen.io.angzarr.router.ffi.v1 import abi_pb2
from ...gen.io.angzarr.v1 import saga_pb2, types_pb2
from ...gen.test.counter import order_saga_angzarr
from ..builders import (
    FQ_INCREASED,
    FQ_RESERVE,
    assert_deferred,
    assert_no_source_component,
    cover_of,
    type_url,
)
from ..fixture import OrderSaga

scenarios("saga.feature")


class _World:
    """One scenario's state: a router with the saga fixture registered (its
    Increased handler recording the source sequence it is given), and the
    dispatch outcome."""

    def __init__(self):
        self.router = Router()
        self.seen: list[int] = []
        saga = order_saga_angzarr.new_order_saga_dispatch(OrderSaga())
        generated = saga.events[FQ_INCREASED]

        def increased(event, dests, source):
            self.seen.append(source.sequence)
            return generated(event, dests, source.cover)

        saga.on_event_with_context(FQ_INCREASED, increased)
        self.router.register_saga(saga)
        self.resp = None
        self.err: CodedError | None = None
        self.refusal: CodedError | None = None

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


def _parity_command():
    """The parity command: cover "inventory", root bytes 10..1f, correlation
    "corr-1"; one page whose command is "/example.Foo" carrying 01020304."""
    cmd = types_pb2.CommandBook()
    cmd.cover.domain = "inventory"
    cmd.cover.root.value = bytes(range(0x10, 0x20))
    cmd.cover.correlation_id = "corr-1"
    page = cmd.pages.add()
    page.command.type_url = "/example.Foo"
    page.command.value = bytes([1, 2, 3, 4])
    return cmd


def _parity_saga() -> SagaDispatch:
    """The parity saga ("order" -> "inventory"): its Increased handler emits
    the parity command twice."""
    return SagaDispatch("parity-saga", "order", ["inventory"]).on_event(
        FQ_INCREASED, lambda _event, _dests, _cover: ([_parity_command(), _parity_command()], [])
    )


def _parity_source(seq: int):
    """One Increased event at ``seq`` under cover "order", root bytes 00..0f,
    correlation "corr-1"."""
    req = _event_source(FQ_INCREASED, seq)
    req.source.cover.root.value = bytes(range(0x10))
    req.source.cover.correlation_id = "corr-1"
    return req


@given("a parity saga emitting the parity command twice")
def _a_parity_saga(world):
    world.router.close()
    world.router = Router()
    world.router.register_saga(_parity_saga())


@given(parsers.re(r'an order saga delivering to "(?P<target>[^"]*)"'))
def _an_order_saga(world, target):
    # Each scenario's fresh saga is registered in _World.__init__.
    pass


@when(parsers.re(r"an Increased event at sequence (?P<seq>\d+) is dispatched"))
def _increased_at(world, seq):
    world.dispatch(_event_source(FQ_INCREASED, int(seq)))


@when(
    parsers.re(
        r'an Increased event of order root "(?P<label>[^"]*)" at sequence (?P<seq>\d+) is dispatched'
    )
)
def _rooted_increased_at(world, label, seq):
    req = _event_source(FQ_INCREASED, int(seq))
    req.source.cover.CopyFrom(cover_of("order", label))
    world.dispatch(req)


@when(parsers.re(r"the parity source event at sequence (?P<seq>\d+) is dispatched"))
def _parity_source_at(world, seq):
    world.dispatch(_parity_source(int(seq)))


@when("a saga declaring a compensation for Reserve is registered")
def _register_compensating_saga(world):
    # The typed SagaDispatch has no way to declare a rejection handler, so the
    # descriptor goes through the binding's low-level registration entry point.
    desc = abi_pb2.SagaDescriptor(name="order-saga", input_domain="order")
    desc.target_domains.append("inventory")
    desc.rejections.append(abi_pb2.RejectionEntry(compensates=FQ_RESERVE, callback_ids=[1]))
    router = Router()
    try:
        router.register_saga_descriptor(desc)
    except CodedError as exc:
        world.refusal = exc
    finally:
        router.close()


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


@then("the command leaves its source component to the coordinator")
def _leaves_source_component(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.commands) == 1
    assert_no_source_component(world.resp.commands[0])


@then(parsers.re(r'the command is deferred from order root "(?P<label>[^"]*)"'))
def _deferred_from_root(world, label):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.commands) == 1
    for page in world.resp.commands[0].pages:
        assert page.header.WhichOneof("sequence_type") == "angzarr_deferred"
        assert page.header.angzarr_deferred.source == cover_of("order", label), (
            "the source is the triggering book's whole cover"
        )


@then(
    parsers.re(r'the command at index (?P<index>\d+) hashes to SHA-256 "(?P<digest>[0-9a-f]{64})"')
)
def _command_hashes(world, index, digest):
    assert world.err is None, f"dispatch failed: {world.err}"
    command = world.resp.commands[int(index)]
    command.DiscardUnknownFields()
    encoded = command.SerializeToString(deterministic=True)
    assert hashlib.sha256(encoded).hexdigest() == digest


@then("the registration is refused as INVALID_ARGUMENT")
def _registration_refused(world):
    assert world.refusal is not None, "the registration was accepted"
    assert world.refusal.grpc == GrpcCode.INVALID_ARGUMENT


@then(parsers.re(r"the saga handler saw source sequence (?P<seq>\d+)"))
def _handler_saw_sequence(world, seq):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert world.seen == [int(seq)]


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
