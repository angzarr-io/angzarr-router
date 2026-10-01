"""Facts, replay and handler-context harness — runs the shared
``conformance/features/context.feature`` against the ledger aggregate, the
reserving process-manager and the tracking projector built through the
binding's hand-written APIs (``on_fact``, ``dispatch_fact``,
``dispatch_replay``, ``CommandContext.cover``, ``on_event_with_cover``, a
full-response PM compensator, ``on_event_with_context``)."""

from __future__ import annotations

import pytest
from pytest_bdd import given, parsers, scenarios, then, when

from ... import (
    AggregateDispatch,
    PageContext,
    ProcessManagerDispatch,
    ProjectorDispatch,
    Rebuilder,
    Router,
    pack,
)
from ...gen.io.angzarr.v1 import command_handler_pb2, process_manager_pb2, types_pb2
from ...gen.test.counter import counter_pb2
from ..builders import (
    FQ_INCREASE_BY,
    FQ_INCREASED,
    FQ_RESERVE,
    assert_deferred,
    cover_of,
    fq_from_url,
    root_of,
    type_url,
)

scenarios("context.feature")


def _increased_page(seq: int | None = None):
    page = types_pb2.EventPage()
    if seq is not None:
        page.header.sequence = seq
    page.event.CopyFrom(pack(counter_pb2.Increased()))
    return page


def _reserve_to(domain: str, fq: str = FQ_RESERVE):
    cmd = types_pb2.CommandBook()
    cmd.cover.domain = domain
    cmd.pages.add().command.type_url = type_url(fq)
    return cmd


# --- the hand-built components ---


def _ledger_aggregate(covers: list) -> AggregateDispatch:
    """The ledger aggregate (domain "ledger") over CounterState: Increased
    folds count += 1; a snapshot loads CounterState; IncreaseBy records the
    handled cover and emits nothing; an Increased fact is annotated as a
    CounterState carrying the folded count."""

    def apply_increased(state, _event):
        state.count += 1

    def load_snapshot(state, snapshot):
        state.ParseFromString(snapshot.value)

    def increase_by(_cmd, _state, cctx):
        covers.append(cctx.cover)

    def annotate(_fact, state):
        return counter_pb2.CounterState(count=state.count)

    rebuilder = (
        Rebuilder(factory=counter_pb2.CounterState)
        .apply(FQ_INCREASED, apply_increased)
        .with_snapshot(load_snapshot)
    )
    return (
        AggregateDispatch("Ledger", "ledger", rebuilder)
        .on_command(FQ_INCREASE_BY, increase_by)
        .on_fact(FQ_INCREASED, annotate)
    )


def _reserving_pm(covers: list) -> ProcessManagerDispatch:
    """The reserving process-manager (domain "reserving-pm", target
    "inventory") over CounterState: an Increased trigger from "counter" records
    the trigger cover and emits nothing; a rejected Reserve is compensated
    with a Release command to "inventory"."""

    def on_increased(_event, _state, _dests, trigger_cover):
        covers.append(trigger_cover)
        return process_manager_pb2.ProcessManagerHandleResponse()

    def on_reserve_rejected(_notification, _rejection, _state):
        resp = process_manager_pb2.ProcessManagerHandleResponse()
        resp.commands.append(_reserve_to("inventory", "test.counter.Release"))
        return resp

    return (
        ProcessManagerDispatch(
            "Reserving",
            "reserving-pm",
            Rebuilder(factory=counter_pb2.CounterState),
            ["inventory"],
        )
        .on_event_with_cover("counter", FQ_INCREASED, on_increased)
        .on_rejected(FQ_RESERVE, on_reserve_rejected)
    )


def _tracking_projector(pages: list) -> ProjectorDispatch:
    """The tracking projector: every Increased fold records its book's root
    and the page's sequence."""

    def fold(_state, _event, ctx: PageContext):
        root = ctx.cover.root.value if ctx.cover is not None else b""
        pages.append((root, ctx.sequence))

    return ProjectorDispatch("Tracker", lambda: None).on_event_with_context(FQ_INCREASED, fold)


# --- requests ---


def _fact_request(fact: str, facts: int, prior: int):
    req = command_handler_pb2.FactRequest()
    req.facts.cover.domain = "ledger"
    for _ in range(facts):
        req.facts.pages.add().event.type_url = type_url("test.counter." + fact)
    for i in range(prior):
        req.prior_events.pages.append(_increased_page(i))
    req.prior_events.next_sequence = prior
    return req


def _replay_request(count: int, events: int):
    req = command_handler_pb2.ReplayRequest()
    req.base_snapshot.sequence = 1
    req.base_snapshot.state.CopyFrom(pack(counter_pb2.CounterState(count=count)))
    for i in range(events):
        req.events.append(_increased_page(2 + i))
    return req


def _ledger_command(label: str):
    cc = types_pb2.ContextualCommand()
    cc.command.cover.CopyFrom(cover_of("ledger", label))
    cc.command.pages.add().command.CopyFrom(pack(counter_pb2.IncreaseBy(n=1)))
    return cc


def _reserving_trigger(label: str, seq: int):
    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.CopyFrom(cover_of("counter", label))
    req.trigger.pages.append(_increased_page(seq))
    return req


def _reserving_rejection(target_domain: str, seq: int):
    rejection = types_pb2.RejectionNotification()
    rejection.rejected_command.CopyFrom(_reserve_to(target_domain))
    notification = types_pb2.Notification()
    notification.payload.CopyFrom(pack(rejection))
    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.domain = "reserving-pm"
    page = req.trigger.pages.add()
    page.header.sequence = seq
    page.event.CopyFrom(pack(notification))
    return req


def _tracked_book(label: str, sequences: list[int]):
    book = types_pb2.EventBook()
    book.cover.CopyFrom(cover_of("counter", label))
    for seq in sequences:
        book.pages.append(_increased_page(seq))
    return book


# --- world ---


class _World:
    def __init__(self):
        self.router = Router()
        self.covers: list = []
        self.pages: list = []
        self.facts = None
        self.replayed = None
        self.pm = None

    def close(self) -> None:
        self.router.close()


@pytest.fixture
def world():
    w = _World()
    yield w
    w.close()


@given("a ledger aggregate")
def _ledger(world):
    world.router.register_aggregate(_ledger_aggregate(world.covers))


@given("a reserving process-manager")
def _reserving(world):
    world.router.register_process_manager(_reserving_pm(world.covers))


@given("a tracking projector")
def _tracking(world):
    world.router.register_projector(_tracking_projector(world.pages))


@when(
    parsers.re(
        r"(?P<facts>\d+) Increased facts are handled over (?P<prior>\d+) prior Increased events"
    )
)
def _increased_facts(world, facts, prior):
    world.facts = world.router.dispatch_fact(_fact_request("Increased", int(facts), int(prior)))


@when("a Reserve fact is handled over no prior events")
def _reserve_fact(world):
    world.facts = world.router.dispatch_fact(_fact_request("Reserve", 1, 0))


@when(
    parsers.re(
        r"the ledger replays a snapshot of (?P<count>\d+) then (?P<events>\d+) Increased events"
    )
)
def _replay(world, count, events):
    resp = world.router.dispatch_replay("ledger", _replay_request(int(count), int(events)))
    assert fq_from_url(resp.state.type_url) == "test.counter.CounterState"
    world.replayed = counter_pb2.CounterState.FromString(resp.state.value)


@when(parsers.re(r'an IncreaseBy command for ledger root "(?P<label>[^"]*)" is dispatched'))
def _ledger_command_step(world, label):
    world.router.dispatch(_ledger_command(label))


@when(
    parsers.re(
        r'an Increased trigger of counter root "(?P<label>[^"]*)" at sequence (?P<seq>\d+) '
        r"is dispatched to the reserving process-manager"
    )
)
def _reserving_trigger_step(world, label, seq):
    world.pm = world.router.dispatch_process_manager(_reserving_trigger(label, int(seq)))


@when(
    parsers.re(
        r'a rejection of Reserve sent to "(?P<domain>[^"]*)" at sequence (?P<seq>\d+) '
        r"is dispatched to the reserving process-manager"
    )
)
def _reserving_rejection_step(world, domain, seq):
    world.pm = world.router.dispatch_process_manager(_reserving_rejection(domain, int(seq)))


@when(
    parsers.re(
        r'Increased events of counter root "(?P<label>[^"]*)" at sequences (?P<first>\d+) '
        r"and (?P<second>\d+) are projected"
    )
)
def _projected(world, label, first, second):
    world.router.dispatch_projector(_tracked_book(label, [int(first), int(second)]))


@then(
    parsers.re(r"(?P<facts>\d+) facts are recorded, each annotated with a count of (?P<count>\d+)")
)
def _annotated(world, facts, count):
    assert len(world.facts.pages) == int(facts)
    for page in world.facts.pages:
        assert fq_from_url(page.event.type_url) == "test.counter.CounterState"
        assert counter_pb2.CounterState.FromString(page.event.value).count == int(count)


@then("the fact is recorded unchanged")
def _unchanged(world):
    assert len(world.facts.pages) == 1
    assert fq_from_url(world.facts.pages[0].event.type_url) == "test.counter.Reserve"


@then(parsers.re(r"the replayed state has a count of (?P<count>\d+)"))
def _replayed(world, count):
    assert world.replayed.count == int(count)


def _single_root(covers: list) -> bytes:
    assert len(covers) == 1, f"the handler ran {len(covers)} times, want once"
    assert covers[0] is not None, "the handler saw no cover"
    return covers[0].root.value


@then(parsers.re(r'the ledger handler saw root "(?P<label>[^"]*)"'))
def _ledger_saw(world, label):
    assert _single_root(world.covers) == root_of(label)


@then(parsers.re(r'the reserving process-manager saw trigger root "(?P<label>[^"]*)"'))
def _pm_saw(world, label):
    assert _single_root(world.covers) == root_of(label)


@then(
    parsers.re(
        r"the reserving process-manager emits one Release command "
        r"deferred from source sequence (?P<seq>\d+)"
    )
)
def _release(world, seq):
    assert len(world.pm.commands) == 1
    command = world.pm.commands[0]
    assert fq_from_url(command.pages[0].command.type_url) == "test.counter.Release"
    assert_deferred(command, "reserving-pm", int(seq), 0)


@then(
    parsers.re(
        r'the projector saw root "(?P<label>[^"]*)" at sequences (?P<first>\d+) and (?P<second>\d+)'
    )
)
def _projector_saw(world, label, first, second):
    root = root_of(label)
    assert world.pages == [(root, int(first)), (root, int(second))]
