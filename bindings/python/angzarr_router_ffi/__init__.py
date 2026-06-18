"""Python binding for the angzarr shared router.

cffi (ABI mode) over the router-ffi C ABI — the same boundary the Go binding
links, consumed here with a genuinely different mechanism (dlopen'd cffi vs
linked cgo) so the ABI is exercised two ways before it freezes (plan §4).

Public surface (engine-shaped, so the unit-6 generator targets it):

    from angzarr_router_ffi import (
        Router, AggregateDispatch, Rebuilder,
        CommandContext, CodedError, reject, GrpcCode, abi_version,
    )
"""

from ._dispatch import (
    AggregateDispatch,
    CodedError,
    CommandContext,
    GrpcCode,
    Rebuilder,
    Router,
    abi_version,
    reject,
)

__all__ = [
    "AggregateDispatch",
    "CodedError",
    "CommandContext",
    "GrpcCode",
    "Rebuilder",
    "Router",
    "abi_version",
    "reject",
]
