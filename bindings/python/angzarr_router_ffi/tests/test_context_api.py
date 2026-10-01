"""The hand-written registration APIs for undo, facts, replay and handler
context: the cover a handler is handling (CommandContext.cover, the PM trigger
cover, the projector page context, and the ``current_cover``/``current_page``
accessors generated handlers reach it through), undo handlers for Compensate
notifications, fact handlers, and Replay state packing."""

from __future__ import annotations

import pytest

from .. import (
    AggregateDispatch,
    CodedError,
    GrpcCode,
    PageContext,
    ProcessManagerDispatch,
    ProjectorDispatch,
    Rebuilder,
    Router,
    SagaDispatch,
    current_cover,
    current_page,
    pack,
)
from ..gen.io.angzarr.v1 import (
    command_handler_pb2,
    process_manager_pb2,
    saga_pb2,
    types_pb2,
)
from ..gen.test.counter import counter_pb2
from . import builders


def _ledger_rebuilder() -> Rebuilder:
    def apply_increased(state, _event):
        state.count += 1

    def load_snapshot(state, snapshot):
        state.ParseFromString(snapshot.value)

    return (
        Rebuilder(factory=counter_pb2.CounterState)
        .apply(builders.FQ_INCREASED, apply_increased)
        .with_snapshot(load_snapshot)
    )


def _increased_page(seq: int):
    page = types_pb2.EventPage()
    page.header.sequence = seq
    page.event.CopyFrom(pack(counter_pb2.Increased()))
    return page


def _ledger_command(label: str):
    cc = types_pb2.ContextualCommand()
    cc.command.cover.CopyFrom(builders.cover_of("ledger", label))
    cc.command.pages.add().command.CopyFrom(pack(counter_pb2.IncreaseBy(n=1)))
    return cc


def _notification_command(domain: str, payload, prior_next: int | None = None):
    notification = types_pb2.Notification()
    notification.payload.CopyFrom(payload)
    cc = types_pb2.ContextualCommand()
    cc.command.cover.domain = domain
    cc.command.pages.add().command.CopyFrom(pack(notification))
    if prior_next is not None:
        cc.events.next_sequence = prior_next
        page = cc.events.pages.add()
        page.header.sequence = prior_next - 1
        page.event.type_url = builders.type_url("test.counter.Unrelated")
    return cc


def _compensate(fq_command: str):
    return pack(types_pb2.Compensate(command_type=fq_command, sequences=[0], reason="aborted"))


def _one_event(name: str):
    resp = command_handler_pb2.BusinessResponse()
    resp.events.pages.add().event.type_url = builders.type_url("test.counter." + name)
    return resp


# --- CommandContext.cover and current_cover() ---


def test_command_handler_reads_its_cover_from_the_context_and_the_accessor():
    seen = []

    def handle(cmd, state, cctx):
        seen.append((cctx.cover.root.value, current_cover().root.value, current_page().sequence))

    dispatch = AggregateDispatch("Ledger", "ledger", _ledger_rebuilder())
    dispatch.on_command(builders.FQ_INCREASE_BY, handle)
    with Router() as router:
        router.register_aggregate(dispatch)
        resp = router.dispatch(_ledger_command("ledger-1"))

    root = builders.root_of("ledger-1")
    assert seen == [(root, root, 0)]
    assert len(resp.events.pages) == 0


def test_current_cover_outside_a_handler_raises():
    with pytest.raises(RuntimeError):
        current_cover()
    with pytest.raises(RuntimeError):
        current_page()


def test_compensator_context_carries_the_aggregate_cover():
    seen = []

    def compensate(notification, rejection, state, cctx):
        seen.append(cctx.cover.domain)
        return _one_event("FundsReleased")

    rejection = types_pb2.RejectionNotification()
    rejection.rejected_command.cover.domain = "inventory"
    rejection.rejected_command.pages.add().command.type_url = builders.type_url(builders.FQ_RESERVE)
    dispatch = AggregateDispatch("Payment", "payment", Rebuilder(factory=lambda: None))
    dispatch.on_rejected("inventory:" + builders.FQ_RESERVE, compensate)
    with Router() as router:
        router.register_aggregate(dispatch)
        resp = router.dispatch(_notification_command("payment", pack(rejection)))

    assert seen == ["payment"]
    assert [builders.fq_from_url(p.event.type_url) for p in resp.events.pages] == [
        "test.counter.FundsReleased"
    ]


# --- undo ---


def _inventory(calls: list) -> AggregateDispatch:
    def undo_adjust(notification, compensate, state, cctx):
        calls.append((compensate.command_type, cctx.next_sequence, cctx.cover.domain))
        return _one_event("StockAdjustmentReverted")

    def undo_reserve(notification, compensate, state, cctx):
        calls.append((compensate.command_type, cctx.next_sequence, cctx.cover.domain))

    return (
        AggregateDispatch("Inventory", "inventory", Rebuilder(factory=lambda: None))
        .on_undo("test.counter.AdjustStock", undo_adjust)
        .on_undo(builders.FQ_RESERVE, undo_reserve)
    )


def test_compensate_routes_to_the_undo_handler_for_its_command_type():
    calls: list = []
    with Router() as router:
        router.register_aggregate(_inventory(calls))
        resp = router.dispatch(
            _notification_command("inventory", _compensate("test.counter.AdjustStock"), 7)
        )

    assert calls == [("test.counter.AdjustStock", 7, "inventory")]
    pages = resp.events.pages
    assert [builders.fq_from_url(p.event.type_url) for p in pages] == [
        "test.counter.StockAdjustmentReverted"
    ]
    assert pages[0].header.sequence == 7


def test_undo_handler_returning_nothing_records_nothing():
    calls: list = []
    with Router() as router:
        router.register_aggregate(_inventory(calls))
        resp = router.dispatch(_notification_command("inventory", _compensate(builders.FQ_RESERVE)))

    assert calls == [(builders.FQ_RESERVE, 0, "inventory")]
    assert len(resp.events.pages) == 0


def test_compensate_without_an_undo_handler_is_unimplemented():
    calls: list = []
    with Router() as router:
        router.register_aggregate(_inventory(calls))
        with pytest.raises(CodedError) as exc:
            router.dispatch(
                _notification_command("inventory", _compensate("test.counter.CountStock"))
            )

    assert calls == []
    assert exc.value.code == "NO_UNDO_HANDLER"
    assert exc.value.grpc == GrpcCode.UNIMPLEMENTED


# --- facts ---


def _fact_request(fq: str, facts: int, prior: int):
    req = command_handler_pb2.FactRequest()
    req.facts.cover.domain = "ledger"
    for _ in range(facts):
        req.facts.pages.add().event.type_url = builders.type_url(fq)
    for i in range(prior):
        req.prior_events.pages.append(_increased_page(i))
    req.prior_events.next_sequence = prior
    return req


def test_fact_handler_annotates_against_rebuilt_state_with_a_message_or_any():
    def annotate_message(fact, state):
        return counter_pb2.CounterState(count=state.count)

    def annotate_any(fact, state):
        return pack(counter_pb2.CounterState(count=state.count + 100))

    for handler, want in ((annotate_message, 3), (annotate_any, 103)):
        dispatch = AggregateDispatch("Ledger", "ledger", _ledger_rebuilder())
        dispatch.on_fact(builders.FQ_INCREASED, handler)
        with Router() as router:
            router.register_aggregate(dispatch)
            book = router.dispatch_fact(_fact_request(builders.FQ_INCREASED, 2, 3))

        assert len(book.pages) == 2
        for page in book.pages:
            assert builders.fq_from_url(page.event.type_url) == "test.counter.CounterState"
            assert counter_pb2.CounterState.FromString(page.event.value).count == want


def test_fact_handler_returning_none_records_the_fact_unchanged():
    seen = []

    def observe(fact, state):
        seen.append(builders.fq_from_url(fact.type_url))

    dispatch = AggregateDispatch("Ledger", "ledger", _ledger_rebuilder())
    dispatch.on_fact(builders.FQ_INCREASED, observe)
    with Router() as router:
        router.register_aggregate(dispatch)
        book = router.dispatch_fact(_fact_request(builders.FQ_INCREASED, 1, 0))

    assert seen == [builders.FQ_INCREASED]
    assert [builders.fq_from_url(p.event.type_url) for p in book.pages] == [builders.FQ_INCREASED]


# --- replay ---


def _replay_request(count: int, events: int):
    req = command_handler_pb2.ReplayRequest()
    req.base_snapshot.sequence = 1
    req.base_snapshot.state.CopyFrom(pack(counter_pb2.CounterState(count=count)))
    for i in range(events):
        req.events.append(_increased_page(2 + i))
    return req


def test_replay_packs_the_rebuilt_state():
    with Router() as router:
        router.register_aggregate(AggregateDispatch("Ledger", "ledger", _ledger_rebuilder()))
        resp = router.dispatch_replay("ledger", _replay_request(10, 2))

    assert resp.state.type_url == "/test.counter.CounterState"
    assert counter_pb2.CounterState.FromString(resp.state.value).count == 12


def test_replay_of_an_aggregate_with_non_message_state_is_not_supported():
    with Router() as router:
        router.register_aggregate(
            AggregateDispatch("Payment", "payment", Rebuilder(factory=lambda: None))
        )
        with pytest.raises(CodedError) as exc:
            router.dispatch_replay("payment", _replay_request(1, 0))

    assert exc.value.code == "NO_HANDLER_REGISTERED"


# --- process-manager trigger cover ---


def _pm_trigger(label: str, seq: int):
    req = process_manager_pb2.ProcessManagerHandleRequest()
    req.trigger.cover.CopyFrom(builders.cover_of("counter", label))
    req.trigger.pages.append(_increased_page(seq))
    return req


def test_process_manager_handler_reads_the_trigger_cover():
    seen = []

    def with_cover(event, state, dests, trigger_cover):
        seen.append(("rich", trigger_cover.root.value, dests.domains()))
        return process_manager_pb2.ProcessManagerHandleResponse()

    def plain(event, state, dests):
        seen.append(("accessor", current_cover().root.value, dests.domains()))
        return process_manager_pb2.ProcessManagerHandleResponse()

    root = builders.root_of("table-1")
    for register, kind in (
        (lambda pm: pm.on_event_with_cover("counter", builders.FQ_INCREASED, with_cover), "rich"),
        (lambda pm: pm.on_event("counter", builders.FQ_INCREASED, plain), "accessor"),
    ):
        seen.clear()
        pm = ProcessManagerDispatch(
            "Reserving", "reserving-pm", Rebuilder(factory=counter_pb2.CounterState), ["inventory"]
        )
        register(pm)
        with Router() as router:
            router.register_process_manager(pm)
            router.dispatch_process_manager(_pm_trigger("table-1", 2))
        assert seen == [(kind, root, ["inventory"])]


# --- saga source cover through the accessor ---


def test_saga_handler_reaches_the_source_cover_through_the_accessor():
    seen = []

    def translate(event, dests, source_cover):
        seen.append((source_cover.root.value, current_cover().root.value))
        return [], []

    saga = SagaDispatch("order-saga", "order").on_event(builders.FQ_INCREASED, translate)
    req = saga_pb2.SagaHandleRequest()
    req.source.cover.CopyFrom(builders.cover_of("order", "order-1"))
    req.source.pages.append(_increased_page(0))
    with Router() as router:
        router.register_saga(saga)
        router.dispatch_saga(req)

    root = builders.root_of("order-1")
    assert seen == [(root, root)]


# --- projector page context ---


def _tracked_book(label: str, sequences: list[int]):
    book = types_pb2.EventBook()
    book.cover.CopyFrom(builders.cover_of("counter", label))
    for seq in sequences:
        book.pages.append(_increased_page(seq))
    return book


def test_projector_fold_reads_each_event_root_and_sequence():
    root = builders.root_of("counter-1")
    rich: list = []
    accessor: list = []

    def fold_rich(state, event, ctx: PageContext):
        rich.append((ctx.cover.root.value, ctx.sequence))

    def fold_plain(state, event):
        accessor.append((current_cover().root.value, current_page().sequence))

    for register in (
        lambda p: p.on_event_with_context(builders.FQ_INCREASED, fold_rich),
        lambda p: p.on_event(builders.FQ_INCREASED, fold_plain),
    ):
        projector = ProjectorDispatch("Tracker", lambda: None)
        register(projector)
        with Router() as router:
            router.register_projector(projector)
            router.dispatch_projector(_tracked_book("counter-1", [4, 5]))

    assert rich == [(root, 4), (root, 5)]
    assert accessor == [(root, 4), (root, 5)]
