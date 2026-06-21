"""P5 property sweep — the binding's breadth signal. Exercises the core's
sequence stamping and rejection threshold across a broad grid of (prior
history, increase amount) the named scenarios don't enumerate. The expected
outcome is the obvious reference model — ``amount`` events stamped at
consecutive sequences continuing from the prior count, or a
VALUE_NOT_POSITIVE rejection at amount 0 — so a divergence flags an off-by-one
in continuation or a wrong threshold. No old client is linked; the shared
conformance features remain the cross-language behavior contract."""

import pytest

from .. import CodedError, Router
from ..gen.test.counter import counter_aggregate_angzarr
from . import builders
from .fixture import CounterAggregate

_BOUND = 13  # 13 x 13 = 169 (prior, amount) combinations


@pytest.fixture
def bare_router():
    r = Router()
    counter_aggregate_angzarr.register_counter_aggregate(r, CounterAggregate())
    yield r
    r.close()


def test_property_sweep(bare_router):
    for prior_n in range(_BOUND):
        for amount in range(_BOUND):
            cmd = builders.increase_command(amount)
            prior = builders.prior_increases(prior_n)
            if prior is not None:
                cmd.events.CopyFrom(prior)

            if amount == 0:
                with pytest.raises(CodedError) as exc:
                    bare_router.dispatch(cmd)
                assert exc.value.code == "VALUE_NOT_POSITIVE", f"prior={prior_n}"
                continue

            resp = bare_router.dispatch(cmd)
            seqs = [p.header.sequence for p in resp.events.pages]
            assert seqs == list(range(prior_n, prior_n + amount)), (
                f"prior={prior_n} amount={amount}"
            )
