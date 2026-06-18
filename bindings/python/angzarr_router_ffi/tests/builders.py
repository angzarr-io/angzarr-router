"""Skeleton-parsing command/event builders, shared by the targeted tests and
the pytest-bdd conformance harness.

Every builder PARSES the orthogonal ``.txtpb`` skeleton first, then sets the
scenario's data BY FIELD on the structured message — the textproto is never
string-templated or altered before parsing (the same discipline the Rust and
Go harnesses follow). Importing this module imports the generated ``_pb2``
packages, which populates the default descriptor pool so ``text_format``
expands the Any payloads in the skeletons.
"""

from __future__ import annotations

import pathlib

from google.protobuf import any_pb2, text_format

from ..gen.io.angzarr.v1 import types_pb2
from ..gen.test.counter import counter_pb2

# The shared conformance fixtures — the same orthogonal envelope skeletons the
# Rust and Go harnesses parse. Resolved relative to this file (repo
# conformance/ tree), not copied.
_FIXTURES_DIR = pathlib.Path(__file__).resolve().parents[4] / "conformance" / "fixtures"

# The framework's canonical type-URL prefix is a bare "/" (router
# TYPE_URL_PREFIX); angzarr-produced URLs take that form, not the
# type.googleapis.com Any default. The core matches prefix-agnostically; the
# binding emits the canonical form. (The google.rpc.ErrorInfo detail in the
# error model is the one exception — pinned to type.googleapis.com.)
_TYPE_URL_PREFIX = "/"

# Fully-qualified type names the CounterAggregate keys on (FIXTURE.md).
FQ_INCREASED = "test.counter.Increased"
FQ_INCREASE_BY = "test.counter.IncreaseBy"
FQ_FAIL_HARD = "test.counter.FailHard"
FQ_RESERVE = "test.counter.Reserve"


def type_url(fq: str) -> str:
    return _TYPE_URL_PREFIX + fq


def fq_from_url(url: str) -> str:
    """The fully-qualified name from a type URL, prefix-agnostic."""
    return url.rsplit("/", 1)[-1]


def _load_skeleton(name: str, message):
    """Read a .txtpb skeleton and parse it into ``message``, expanding Any
    payloads via the default proto registry."""
    text = (_FIXTURES_DIR / name).read_text()
    text_format.Parse(text, message)
    return message


def _inner_command_any(cc) -> any_pb2.Any:
    return cc.command.pages[0].command


def increase_command(n: int):
    """Parse the IncreaseBy skeleton, then set ``n`` on the inner message by
    field (decode the Any payload, set n, re-encode)."""
    cc = _load_skeleton("command_increase.txtpb", types_pb2.ContextualCommand())
    any_msg = _inner_command_any(cc)
    inner = counter_pb2.IncreaseBy()
    inner.ParseFromString(any_msg.value)
    inner.n = n
    any_msg.value = inner.SerializeToString()
    return cc


def fail_hard_command():
    """Parse the FailHard skeleton (no scenario data)."""
    return _load_skeleton("command_failhard.txtpb", types_pb2.ContextualCommand())


def unhandled_command():
    """Parse the Reserve skeleton — a command with no registered handler
    (drives NO_HANDLER_REGISTERED before rebuild)."""
    return _load_skeleton("command_unhandled.txtpb", types_pb2.ContextualCommand())


def parent_linkage() -> any_pb2.Any:
    """An opaque fill-only ext stamped on a command's cover, to prove ext
    propagation onto emitted events."""
    return any_pb2.Any(type_url=type_url("test.counter.Parent"), value=bytes([1, 2, 3]))


def increase_command_with_linkage(n: int):
    """Set parent linkage on a parsed command's cover."""
    cc = increase_command(n)
    cc.command.cover.ext.CopyFrom(parent_linkage())
    return cc


def rejection_command(fq_command: str):
    """Wrap a rejection Notification for ``fq_command`` into a
    ContextualCommand, routed through the same dispatch entry — the core
    detects the notification type and takes the compensation path. Built by
    field; the envelope nests Notification -> RejectionNotification -> the
    rejected book."""
    rejection = types_pb2.RejectionNotification()
    rejection.rejected_command.cover.domain = "counter"
    page = rejection.rejected_command.pages.add()
    page.command.type_url = type_url(fq_command)

    notification = types_pb2.Notification()
    notification.payload.type_url = type_url("io.angzarr.v1.RejectionNotification")
    notification.payload.value = rejection.SerializeToString()

    cc = types_pb2.ContextualCommand()
    cc.command.cover.domain = "counter"
    cmd_page = cc.command.pages.add()
    cmd_page.command.type_url = type_url("io.angzarr.v1.Notification")
    cmd_page.command.value = notification.SerializeToString()
    return cc


# Envelope-guard negatives: a well-formed parsed command with exactly one
# structural field cleared, so the guard fires regardless of the rest.


def command_missing_book():
    cc = increase_command(1)
    cc.ClearField("command")
    return cc


def command_missing_page():
    cc = increase_command(1)
    cc.command.ClearField("pages")
    return cc


def command_missing_payload():
    cc = increase_command(1)
    cc.command.pages[0].ClearField("payload")
    return cc


def _increased_page_at(seq: int):
    """Parse the Increased event skeleton and stamp a sequence."""
    page = _load_skeleton("event_increased.txtpb", types_pb2.EventPage())
    page.header.sequence = seq
    return page


def prior_increases(n: int):
    """Replay the parsed Increased skeleton at consecutive sequences 0..n-1,
    with the next sequence the core derives. None for an empty history."""
    if n == 0:
        return None
    book = types_pb2.EventBook(next_sequence=n)
    for i in range(n):
        book.pages.append(_increased_page_at(i))
    return book


def corrupt_history():
    """One parsed Increased page whose payload is overwritten with an
    undecodable varint, so the fold fails (PERSISTED_EVENT_CORRUPT)."""
    page = _increased_page_at(0)
    page.event.value = bytes([0xFF, 0xFF, 0xFF])
    return types_pb2.EventBook(pages=[page], next_sequence=1)


def snapshot_history():
    """Seed count 10 at sequence 10, plus a covered page (10, skipped) and an
    uncovered page (11, applied) — a rebuild observes 11."""
    book = types_pb2.EventBook(next_sequence=12)
    book.snapshot.sequence = 10
    book.snapshot.state.type_url = type_url("test.counter.CounterState")
    book.snapshot.state.value = counter_pb2.CounterState(count=10).SerializeToString()
    book.pages.append(_increased_page_at(10))
    book.pages.append(_increased_page_at(11))
    return book
