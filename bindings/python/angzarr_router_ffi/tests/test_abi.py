"""P1 smoke test: the cffi/dlopen foundation loads and the ABI version
matches. Proves codegen of the C ABI and the dlopen of the cdylib before any
dispatch logic is exercised."""

from .. import abi_version


def test_abi_version_is_one():
    assert abi_version() == 1
