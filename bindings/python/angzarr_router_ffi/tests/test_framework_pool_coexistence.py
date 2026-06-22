"""Guardrail for the framework-proto descriptor-pool invariant.

The binding and any consumer (an examples app, another binding) both generate
the SAME shared framework contract (io.angzarr.v1.*, sererr) and register it
into protobuf's process-global ``descriptor_pool.Default()``. protobuf keys the
pool by proto FILE NAME and tolerates re-registration only when the copies are
byte-identical (upb ``IsEqualByDef``); a divergent copy of the same file name
raises ``duplicate file name``. So coexistence rests on every generator
producing byte-identical descriptors for the shared files.

The one thing that silently breaks that is a consumer-specific ``FileOptions``
value (notably ``go_package``) baked into the serialized FileDescriptorProto:
buf managed mode embeds it, so a binding-specific override makes the binding's
shared descriptors diverge from every other party. ``buf.gen.yaml`` therefore
DISABLES go_package management on the shared framework paths so their native
(contract) value passes through identically everywhere.

These tests are hermetic (no buf, no network): they guard the invariant on the
already-generated descriptors and pin the upb dedup semantics. The end-to-end
"generate the contract twice and diff the bytes" check lives in angzarr-cli,
the single deterministic generator. See framework-proto-collision.design.md.
"""

from __future__ import annotations

import pytest
from google.protobuf import descriptor_pb2, descriptor_pool

from ..gen.io.angzarr.v1 import (
    command_handler_pb2,
    process_manager_pb2,
    saga_pb2,
    types_pb2,
)
from ..gen.sererr.v1 import sererr_pb2

# The binding-specific go_package the buf.gen.yaml override applies. It MUST NOT
# reach the shared framework descriptors, or their bytes diverge from a
# consumer's copy and collide in Default(). (abi, test.counter are binding-only
# — no consumer generates them — so they keep the override.)
_BINDING_OVERRIDE_MARKER = "angzarr-router/bindings"

# Files that make up the shared contract every consumer also generates.
_SHARED_FRAMEWORK = [
    sererr_pb2,
    types_pb2,
    command_handler_pb2,
    saga_pb2,
    process_manager_pb2,
]


@pytest.mark.parametrize("module", _SHARED_FRAMEWORK, ids=lambda m: m.DESCRIPTOR.name)
def test_shared_framework_go_package_is_consumer_neutral(module):
    """A shared framework descriptor must carry its native go_package, not the
    binding's override — otherwise its serialized bytes differ from a
    consumer's identical-contract copy and collide in the global pool."""
    go_package = module.DESCRIPTOR.GetOptions().go_package
    assert _BINDING_OVERRIDE_MARKER not in go_package, (
        f"{module.DESCRIPTOR.name} carries a binding-specific go_package "
        f"({go_package!r}); buf.gen.yaml must keep go_package native on shared "
        f"framework paths so independently-generated copies stay byte-identical."
    )


def test_identical_recopy_dedups_in_default():
    """A byte-identical second copy of a shared file (what a consumer using the
    same toolchain produces) is deduplicated by the global pool, not rejected —
    the mechanism the whole design relies on."""
    serialized = sererr_pb2.DESCRIPTOR.serialized_pb
    # Importing sererr_pb2 already registered this exact file in Default(); a
    # byte-identical re-add must be a no-op rather than a collision.
    descriptor_pool.Default().AddSerializedFile(serialized)


def test_divergent_copy_collides_in_default():
    """A divergent copy of the same file name (e.g. a leaked consumer-specific
    go_package) DOES collide — proving this guard would catch toolchain drift,
    and documenting the exact failure mode the consumer-neutral check prevents."""
    fdp = descriptor_pb2.FileDescriptorProto()
    fdp.ParseFromString(sererr_pb2.DESCRIPTOR.serialized_pb)
    fdp.options.go_package = "github.com/divergent-consumer/whatever/sererr/v1;sererrv1"

    with pytest.raises(TypeError, match="duplicate file name"):
        descriptor_pool.Default().AddSerializedFile(fdp.SerializeToString())
