"""P4 conformance harness — the shared cross-language behavior suite. Runs the
same ``conformance/features/counter.feature`` (resolved via
``bdd_features_base_dir``) the Rust cucumber-rs and Go godog harnesses run,
against the Python binding via pytest-bdd. Only the step layer is new; the
behavior spec is shared, unchanged."""

from __future__ import annotations

import pytest
from pytest_bdd import given, parsers, scenarios, then, when

from ... import CodedError, Router
from ...gen.test.counter import counter_angzarr
from .. import builders
from ..fixture import CounterAggregate, Observation

scenarios("counter.feature")


class _World:
    """One scenario's state: a router with the fixture registered, the prior
    book to supply, and the dispatch outcome."""

    def __init__(self):
        self.observed: list[Observation] = []
        self.router = Router()
        counter_angzarr.register_counter_aggregate(self.router, CounterAggregate(self.observed))
        self.prior = None
        self.resp = None
        self.err: CodedError | None = None

    def dispatch(self, cc) -> None:
        if self.prior is not None:
            cc.events.CopyFrom(self.prior)
        try:
            self.resp = self.router.dispatch(cc)
            self.err = None
        except CodedError as exc:
            self.err = exc
            self.resp = None

    def last_observed(self) -> Observation:
        assert self.err is None, f"dispatch failed: {self.err}"
        assert self.observed, "the handler recorded no observation"
        return self.observed[-1]

    def close(self) -> None:
        self.router.close()


@pytest.fixture
def world():
    w = _World()
    yield w
    w.close()


# --- Given: the prior-history the next dispatch rebuilds over ---


@given("a new counter")
def _new_counter(world):
    world.prior = None


@given(parsers.re(r"a counter that has already recorded (?P<n>\d+) increases?"))
def _recorded_increases(world, n):
    world.prior = builders.prior_increases(int(n))


@given("a counter whose history holds a corrupt event")
def _corrupt_history(world):
    world.prior = builders.corrupt_history()


@given("a counter restored from a snapshot of 10 with one newer event")
def _snapshot_history(world):
    world.prior = builders.snapshot_history()


# --- When: dispatch a command ---


@when(parsers.parse("the operator increases the counter by {n:d}"))
def _increase_by(world, n):
    world.dispatch(builders.increase_command(n))


@when(parsers.parse("the operator increases the counter by {n:d} on behalf of a parent"))
def _increase_on_behalf(world, n):
    world.dispatch(builders.increase_command_with_linkage(n))


@when("the operator triggers a hard failure")
def _hard_failure(world):
    world.dispatch(builders.fail_hard_command())


@when("an unhandled command is dispatched")
def _unhandled(world):
    world.dispatch(builders.unhandled_command())


@when("a command with no command book is dispatched")
def _no_book(world):
    world.dispatch(builders.command_missing_book())


@when("a command with an empty command book is dispatched")
def _empty_book(world):
    world.dispatch(builders.command_missing_page())


@when("a command whose page carries no payload is dispatched")
def _no_payload(world):
    world.dispatch(builders.command_missing_payload())


@when("a Reserve command is rejected")
def _reserve_rejected(world):
    world.dispatch(builders.rejection_command("test.counter.Reserve"))


@when("an unregistered command is rejected")
def _unregistered_rejected(world):
    world.dispatch(builders.rejection_command("test.counter.Undeclared"))


# --- Then: assert the outcome ---


def _assert_recorded(world, count: int, start: int) -> None:
    assert world.err is None, f"dispatch failed: {world.err}"
    pages = world.resp.events.pages
    assert len(pages) == count, f"recorded {len(pages)} events, want {count}"
    for i, page in enumerate(pages):
        assert page.header.sequence == start + i, f"event {i} at sequence {page.header.sequence}"


@then(parsers.parse("{count:d} increases are recorded, starting at sequence {start:d}"))
def _recorded_starting(world, count, start):
    _assert_recorded(world, count, start)


@then(parsers.parse("{count:d} increases are recorded, continuing from sequence {start:d}"))
def _recorded_continuing(world, count, start):
    _assert_recorded(world, count, start)


@then(parsers.parse("the command is rejected as {code}"))
def _rejected_as(world, code):
    assert world.err is not None, f"expected coded error {code}, got a response"
    assert world.err.code == code, f"code = {world.err.code}, want {code}"


@then(parsers.parse("the command fails with {code}"))
def _fails_with(world, code):
    assert world.err is not None, f"expected coded error {code}, got a response"
    assert world.err.code == code, f"code = {world.err.code}, want {code}"


@then("no events are recorded")
def _no_events(world):
    # A rejected command produces no response at all (resp is None) — that is
    # "no events recorded" just as much as an empty events book is.
    pages = world.resp.events.pages if world.resp is not None else []
    assert len(pages) == 0


@then("the recorded events carry the parent linkage")
def _carry_parent_linkage(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    ext = world.resp.events.cover.ext
    assert ext == builders.parent_linkage(), f"cover ext = {ext}, want parent linkage"


@then("the compensations run first then second")
def _compensations_in_order(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    pages = world.resp.events.pages
    want = ["test.counter.CompensatedFirst", "test.counter.CompensatedSecond"]
    got = [builders.fq_from_url(p.event.type_url) for p in pages]
    assert got == want, f"compensations = {got}, want {want}"


@then("no compensation is recorded")
def _no_compensation(world):
    assert world.err is None, f"dispatch failed: {world.err}"
    assert len(world.resp.events.pages) == 0


@then(parsers.parse("the handler saw no prior history, at next sequence {seq:d}"))
def _handler_saw_no_prior(world, seq):
    obs = world.last_observed()
    assert obs.cctx.had_prior_events is False
    assert obs.cctx.next_sequence == seq


@then(parsers.parse("the handler saw prior history, at next sequence {seq:d}"))
def _handler_saw_prior(world, seq):
    obs = world.last_observed()
    assert obs.cctx.had_prior_events is True
    assert obs.cctx.next_sequence == seq


@then(parsers.parse("the handler saw a counter of {count:d}, at next sequence {seq:d}"))
def _handler_saw_counter(world, count, seq):
    obs = world.last_observed()
    assert obs.count == count, f"observed counter = {obs.count}, want {count}"
    assert obs.cctx.next_sequence == seq
