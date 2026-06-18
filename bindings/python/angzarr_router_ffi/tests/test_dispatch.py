"""P2 targeted marshaling/error tests — what the Rust ABI consumer test pins,
re-confirmed across the cffi seam: the command round trip and sequence
stamping, the coded-rejection and unclassified-failure error model, and the
CommandContextAux decode with applier execution during rebuild."""

import pytest

from .. import CodedError
from . import builders
from .fixture import Observation


def test_increase_by_emits_sequenced_events(router):
    """A command emits one event per unit and the core stamps consecutive
    sequences — the full round trip across the seam."""
    resp = router.dispatch(builders.increase_command(3))
    seqs = [p.header.sequence for p in resp.events.pages]
    assert seqs == [0, 1, 2]


def test_increase_by_zero_is_coded(router):
    """A coded business rejection crosses back as a CodedError carrying the
    stable reason — the google.rpc.Status/ErrorInfo round trip."""
    with pytest.raises(CodedError) as exc:
        router.dispatch(builders.increase_command(0))
    assert exc.value.code == "VALUE_NOT_POSITIVE"


def test_fail_hard_is_unhandled(router):
    """An unclassified handler error is classified by the binding as
    UNHANDLED_HANDLER_ERROR before it crosses the seam."""
    with pytest.raises(CodedError) as exc:
        router.dispatch(builders.fail_hard_command())
    assert exc.value.code == "UNHANDLED_HANDLER_ERROR"


def test_prior_events_reach_handler_context(router, observed: list[Observation]):
    """Prior events fold into state and reach the handler as historical
    evidence — the CommandContextAux decode plus applier execution across the
    seam. A fresh aggregate reports no prior history."""
    router.dispatch(builders.increase_command(1))
    fresh = observed[0]
    assert fresh.cctx.had_prior_events is False
    assert fresh.cctx.next_sequence == 0
    assert fresh.count == 0

    cmd = builders.increase_command(1)
    cmd.events.CopyFrom(builders.prior_increases(2))
    router.dispatch(cmd)
    prior = observed[1]
    assert prior.cctx.had_prior_events is True
    assert prior.cctx.next_sequence == 2
    assert prior.count == 2
