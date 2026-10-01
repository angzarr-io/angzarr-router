"""Python binding for the angzarr shared router.

cffi (ABI mode) over the router-ffi C ABI — the same boundary the Go binding
links, consumed here with a genuinely different mechanism (dlopen'd cffi vs
linked cgo) so the ABI is exercised two ways before it freezes (plan §4).

Public surface (engine-shaped, so the unit-6 generator targets it):

    from angzarr_router_ffi import (
        Router, AggregateDispatch, Rebuilder, ProjectorDispatch,
        SagaDispatch, ProcessManagerDispatch, Destinations,
        CommandContext, PageContext, current_cover, current_page,
        CodedError, reject, GrpcCode, abi_version, AbiVersionError,
    )
"""

from ._abi import AbiVersionError
from ._dispatch import (
    AggregateDispatch,
    CodedError,
    CommandContext,
    Destinations,
    GrpcCode,
    PageContext,
    ProcessManagerDispatch,
    ProjectorDispatch,
    Rebuilder,
    Router,
    SagaDispatch,
    abi_version,
    any_decode_error,
    current_cover,
    current_page,
    pack,
    reject,
)

__all__ = [
    "AbiVersionError",
    "AggregateDispatch",
    "CodedError",
    "CommandContext",
    "Destinations",
    "GrpcCode",
    "PageContext",
    "ProcessManagerDispatch",
    "ProjectorDispatch",
    "Rebuilder",
    "Router",
    "SagaDispatch",
    "abi_version",
    "any_decode_error",
    "current_cover",
    "current_page",
    "pack",
    "reject",
]
