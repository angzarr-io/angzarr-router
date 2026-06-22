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

import re
from pathlib import Path

import pytest
from google.protobuf import descriptor_pb2, descriptor_pool

from ..gen.io.angzarr.v1 import (
    command_handler_pb2,
    process_manager_pb2,
    saga_pb2,
    types_pb2,
)
from ..gen.sererr.v1 import sererr_pb2

# The string/bool FileOptions buf managed mode can rewrite. Managed mode injects
# values derived from the proto package (java_package, csharp_namespace, …) —
# but the contract DECLARES some of these natively, so "must be empty" is wrong.
# The correct invariant: the descriptor's file options equal exactly what the
# .proto source declares. Any extra/changed option is a managed-mode mutation,
# embedded in serialized_pb, that breaks byte-identity with a consumer not
# running managed mode.
_FILE_OPTION_FIELDS = [
    "go_package",
    "java_package",
    "java_outer_classname",
    "java_multiple_files",
    "csharp_namespace",
    "objc_class_prefix",
    "php_namespace",
    "php_class_prefix",
    "php_metadata_namespace",
    "ruby_package",
    "swift_prefix",
]

# Files that make up the shared contract every consumer also generates.
_SHARED_FRAMEWORK = [
    sererr_pb2,
    types_pb2,
    command_handler_pb2,
    saga_pb2,
    process_manager_pb2,
]


def _proto_root() -> Path:
    """The angzarr-project proto root, found by walking up from this test."""
    for ancestor in Path(__file__).resolve().parents:
        candidate = ancestor / "angzarr-project" / "proto"
        if candidate.is_dir():
            return candidate
    pytest.skip("angzarr-project/proto not available")


def _declared_file_options(proto_path: Path) -> dict[str, object]:
    """Parse the top-level (file-scope) options the .proto source declares."""
    declared: dict[str, object] = {}
    depth = 0
    for line in proto_path.read_text().splitlines():
        stripped = line.strip()
        if depth == 0 and stripped.startswith("option "):
            match = re.match(r"option\s+([\w.]+)\s*=\s*(.+);", stripped)
            if match and match.group(1) in _FILE_OPTION_FIELDS:
                raw = match.group(2).strip()
                if raw.startswith('"') and raw.endswith('"'):
                    declared[match.group(1)] = raw[1:-1]
                elif raw in ("true", "false"):
                    declared[match.group(1)] = raw == "true"
        depth += stripped.count("{") - stripped.count("}")
    return declared


def _descriptor_file_options(options) -> dict[str, object]:
    return {f: getattr(options, f) for f in _FILE_OPTION_FIELDS if options.HasField(f)}


@pytest.mark.parametrize("module", _SHARED_FRAMEWORK, ids=lambda m: m.DESCRIPTOR.name)
def test_shared_framework_descriptors_are_native(module):
    """A shared framework descriptor's file options must equal exactly what its
    .proto declares — no managed-mode injection. Managed mode rewrites not just
    go_package but java_package/csharp_namespace/etc.; any divergence from the
    source means the serialized bytes differ from a consumer that doesn't run
    managed mode, and the two copies collide in the global pool. buf.gen.yaml
    must disable ALL managed options on the shared framework paths (a disable
    entry with only `path:` and no `file_option:`)."""
    name = module.DESCRIPTOR.name
    actual = _descriptor_file_options(module.DESCRIPTOR.GetOptions())
    expected = _declared_file_options(_proto_root() / name)

    assert actual == expected, (
        f"{name} file options diverge from the .proto source.\n"
        f"  descriptor: {actual}\n  declared:   {expected}\n"
        f"Extra/changed options are managed-mode mutations baked into "
        f"serialized_pb that break byte-identity with a consumer not running "
        f"managed mode."
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
