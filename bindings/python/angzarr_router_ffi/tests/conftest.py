"""Shared pytest fixtures for the binding tests: a router with the
CounterAggregate fixture registered, and the handler's observation log."""

from __future__ import annotations

import pytest

from .. import Router
from .fixture import Observation, counter_aggregate


@pytest.fixture
def observed() -> list[Observation]:
    """The handler's observation log — what each command handler saw."""
    return []


@pytest.fixture
def router(observed: list[Observation]):
    """A router with the CounterAggregate fixture registered. Closed after
    each test."""
    r = Router()
    r.register_aggregate(counter_aggregate(observed))
    yield r
    r.close()
