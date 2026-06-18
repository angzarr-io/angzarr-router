"""Projector conformance harness — runs the shared
``conformance/features/projector.feature`` (the same one the Rust cucumber-rs
and Go godog harnesses run) against the Python binding via pytest-bdd. Only the
step layer is new; the behavior spec is shared, unchanged."""

from __future__ import annotations

import pytest
from pytest_bdd import given, parsers, scenarios, then, when

from ... import CodedError, Router
from ...gen.io.angzarr.v1 import types_pb2
from ..builders import FQ_INCREASED, type_url
from ..fixture import counter_projector

scenarios("projector.feature")


class _World:
    """One scenario's state: a router with the projector fixture registered,
    and the dispatch outcome."""

    def __init__(self):
        self.router = Router()
        self.router.register_projector(counter_projector())
        self.proj = None
        self.err: CodedError | None = None

    def dispatch(self, book) -> None:
        try:
            self.proj = self.router.dispatch_projector(book)
            self.err = None
        except CodedError as exc:
            self.err = exc
            self.proj = None

    def close(self) -> None:
        self.router.close()


@pytest.fixture
def world():
    w = _World()
    yield w
    w.close()


def _delivery(domain: str, n: int):
    """An EventBook of n Increased events whose cover carries domain."""
    book = types_pb2.EventBook()
    book.cover.domain = domain
    for _ in range(n):
        book.pages.add().event.type_url = type_url(FQ_INCREASED)
    return book


@given("a counter projection")
def _a_counter_projection(world):
    # Each scenario's fresh projector is registered in _World.__init__.
    pass


@when(parsers.re(r'(?P<n>\d+) events are delivered in domain "(?P<domain>[^"]*)"'))
def _events_delivered(world, n, domain):
    world.dispatch(_delivery(domain, int(n)))


@when("a delivery arrives with no cover")
def _delivery_no_cover(world):
    book = _delivery("counter", 1)
    book.ClearField("cover")
    world.dispatch(book)


@then(parsers.re(r"the projection records (?P<n>\d+) events?"))
def _records(world, n):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert world.proj.sequence == int(n)


@then("the projection records nothing")
def _records_nothing(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert world.proj.sequence == 0


@then(parsers.re(r"the delivery fails with (?P<code>[A-Z_]+)"))
def _fails_with(world, code):
    assert world.err is not None, f"expected failure {code}, got a success"
    assert world.err.code == code
