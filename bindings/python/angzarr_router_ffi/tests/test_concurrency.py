"""P5 GIL-threaded concurrent dispatch. Drives parallel dispatches on distinct
sessions from multiple threads and asserts per-session isolation: each
dispatch creates its own host_ctx session, so the emitted sequence reflects
only that dispatch's own prior history — never another thread's. A
Send/Sync or session-bleed defect surfaces here, through the cffi callback's
GIL acquisition."""

import threading

from .. import Router
from ..gen.test.counter import counter_aggregate_angzarr
from . import builders
from .fixture import CounterAggregate

_THREADS = 8
_ITERATIONS = 50


def test_concurrent_dispatches_isolate_sessions():
    router = Router()
    counter_aggregate_angzarr.register_counter_aggregate(router, CounterAggregate())
    errors: list[Exception] = []

    def worker(prior_n: int) -> None:
        try:
            prior = builders.prior_increases(prior_n)
            for _ in range(_ITERATIONS):
                cmd = builders.increase_command(1)
                if prior is not None:
                    cmd.events.CopyFrom(prior)
                resp = router.dispatch(cmd)
                seq = resp.events.pages[0].header.sequence
                if seq != prior_n:
                    raise AssertionError(f"thread prior={prior_n} saw sequence {seq}")
        except Exception as exc:  # noqa: BLE001 — collected and re-raised on the main thread
            errors.append(exc)

    threads = [threading.Thread(target=worker, args=(n,)) for n in range(_THREADS)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    router.close()

    assert not errors, errors
