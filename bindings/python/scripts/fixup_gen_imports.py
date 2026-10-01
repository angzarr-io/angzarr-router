#!/usr/bin/env python3
"""Rewrite generated protobuf imports onto the binding's gen package prefix.

The protocolbuffers/python plugin emits cross-file imports by proto PATH, so
a file under io/angzarr/v1/ imports a sibling as
``from io.angzarr.v1 import types_pb2``. Those top-level package names cannot
be imported as written:

- ``io`` is a stdlib module already loaded (and not a package) before any
  user code runs, so ``from io.angzarr...`` raises "io is not a package";
- ``test`` and ``sererr`` would become top-level packages polluting the
  global namespace.

So after generation we reroot the binding's own packages under
``angzarr_router_ffi.gen.`` (where the tree actually lives). ``google.*``
imports are left untouched: google.protobuf is the runtime and
google.rpc/google.api come from googleapis-common-protos.

Run by ``just py-binding-gen`` after ``buf generate``; the rewritten tree is
committed.
"""

import pathlib
import re
import sys

# Proto roots the binding generates and therefore must reroot. `google` is
# deliberately absent — it resolves from installed runtime packages.
GEN_ROOTS = ("io", "sererr", "test")
PACKAGE_PREFIX = "angzarr_router_ffi.gen."

# Matches the gencode import forms at line start: `from io.…`, `import io.…`.
_PATTERN = re.compile(
    r"^(from|import) (" + "|".join(GEN_ROOTS) + r")\.",
    re.MULTILINE,
)


def fixup(gen_dir: pathlib.Path) -> int:
    rewritten = 0
    # *_pb2.py / *_pb2_grpc.py: protoc-gen-python's own cross-file imports.
    # *_angzarr.py: the angzarr codegen wiring, which imports framework types
    # and message modules by the same proto-path form and so needs the same
    # reroot (its `import angzarr_router_ffi as _az` is left untouched — it is
    # not under a GEN_ROOT).
    globs = ("*_pb2.py", "*_pb2_grpc.py", "*_angzarr.py")
    paths: list[pathlib.Path] = []
    for pattern in globs:
        paths.extend(sorted(gen_dir.rglob(pattern)))
    for path in paths:
        text = path.read_text()
        new = _PATTERN.sub(rf"\1 {PACKAGE_PREFIX}\2.", text)
        if new != text:
            path.write_text(new)
            rewritten += 1
    return rewritten


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: fixup_gen_imports.py <gen-dir>", file=sys.stderr)
        return 2
    gen_dir = pathlib.Path(sys.argv[1])
    if not gen_dir.is_dir():
        print(f"not a directory: {gen_dir}", file=sys.stderr)
        return 1
    count = fixup(gen_dir)
    print(f"rewrote imports in {count} generated files under {gen_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
