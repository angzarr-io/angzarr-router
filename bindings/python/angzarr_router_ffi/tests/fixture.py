"""The conformance fixtures in Python, implementing the angzarr-generated
Handler protocols (gen/test/counter/*_angzarr.py). Same behavior the Rust
core and Go binding implement; the dispatch wiring is now generated, so these
are the proof the generated Python seam is faithful. Registered via the
generated register_* helpers in each scenario's world.

``observed``, when supplied, records the CommandContext and rebuilt count each
command handler saw — the historical-state evidence the suite asserts, since
state never crosses the boundary."""

from __future__ import annotations

from dataclasses import dataclass

from .. import CommandContext, reject
from ..gen.io.angzarr.v1 import command_handler_pb2, process_manager_pb2, types_pb2
from ..gen.test.counter import counter_pb2
from .builders import FQ_RESERVE, type_url


@dataclass
class Observation:
    """One record of what a command handler saw: the context and rebuilt
    state count."""

    cctx: CommandContext
    count: int


# --- CounterAggregate ---


class CounterAggregate:
    """Implements counter_aggregate_angzarr.CounterAggregateHandler. The rebuilder
    (generated) folds Increased via apply_increased and seeds snapshots
    generically; the typed-emit handler returns Increased events the wiring
    packs into the EventBook."""

    def __init__(self, observed: list[Observation] | None = None):
        self.observed = observed

    def increase_by(self, cmd, state, cctx: CommandContext):
        if self.observed is not None:
            self.observed.append(Observation(cctx=cctx, count=state.count))
        if cmd.n == 0:
            raise reject("VALUE_NOT_POSITIVE", "increase amount must be positive")
        return [counter_pb2.Increased() for _ in range(cmd.n)]

    def fail_hard(self, cmd, state, cctx: CommandContext):
        raise RuntimeError("hard failure")

    def apply_increased(self, state, event) -> None:
        state.count += 1

    def on_reserve_rejected(self, notification, rejection, state, cctx: CommandContext):
        # Within-component fan-out collapses to one compensator (subscriber =
        # component): append both ordered markers in one response.
        resp = command_handler_pb2.BusinessResponse()
        for name in ("CompensatedFirst", "CompensatedSecond"):
            resp.events.pages.add().event.type_url = type_url("test.counter." + name)
        return resp


# --- OrderSaga ---


class OrderSaga:
    """Implements order_saga_angzarr.OrderSagaHandler."""

    def increased(self, event, dests):
        cmd = _reserve_command()
        if dests.has("inventory"):
            dests.stamp_command(cmd, "inventory")
        return [cmd], []

    def on_reserve_rejected(self, notification, rejection):
        return [_one_fact()]


# --- CounterProjector ---


class CounterProjector:
    """Implements counter_projector_angzarr.CounterProjectorHandler."""

    def increased(self, projection, event) -> None:
        projection.count += 1

    def finish(self, projection, events):
        proj = types_pb2.Projection()
        if events.HasField("cover"):
            proj.cover.CopyFrom(events.cover)
        proj.projector = "counter-projector"
        proj.sequence = projection.count
        return proj


# --- OrderProcessManager ---


class OrderProcessManager:
    """Implements order_process_manager_angzarr.OrderProcessManagerHandler."""

    def increased(self, event, state, dests):
        cmd = _reserve_command()
        if dests.has("inventory"):
            dests.stamp_command(cmd, "inventory")
        resp = process_manager_pb2.ProcessManagerHandleResponse()
        resp.commands.append(cmd)
        for _ in range(state.count):
            resp.facts.add().pages.add()
        return resp

    def apply_increased(self, state, event) -> None:
        state.count += 1

    def on_reserve_rejected(self, notification, rejection, state):
        escalation = types_pb2.Notification()
        escalation.cover.domain = "escalated"
        return [_one_fact()], escalation


def _reserve_command():
    cmd = types_pb2.CommandBook()
    cmd.cover.domain = "inventory"
    cmd.pages.add().command.type_url = type_url(FQ_RESERVE)
    return cmd


def _one_fact():
    event = types_pb2.EventBook()
    event.pages.add()
    return event
