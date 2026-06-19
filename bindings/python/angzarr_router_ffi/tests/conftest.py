"""Shared pytest fixtures for the binding tests: a router with the
CounterAggregate fixture registered, and the handler's observation log."""

from __future__ import annotations

import pytest

from .. import Router
from ..gen.test.counter import counter_angzarr
from .fixture import CounterAggregate, Observation


@pytest.fixture
def observed() -> list[Observation]:
    """The handler's observation log — what each command handler saw."""
    return []


@pytest.fixture
def router(observed: list[Observation]):
    """A router with the CounterAggregate fixture registered. Closed after
    each test."""
    r = Router()
    counter_angzarr.register_counter_aggregate(r, CounterAggregate(observed))
    yield r
    r.close()
