"""The cffi/dlopen foundation loads and the binding refuses a router-ffi
library whose ABI version drifted from the one it was built against."""

import pytest

from .. import AbiVersionError, abi_version
from .._abi import EXPECTED_ABI_VERSION, check_abi_version


def test_abi_version_is_three():
    assert abi_version() == 3
    assert EXPECTED_ABI_VERSION == 3


def test_matching_abi_version_is_accepted():
    assert check_abi_version(EXPECTED_ABI_VERSION) is None


def test_drifted_abi_version_is_refused_naming_both_versions():
    with pytest.raises(AbiVersionError) as exc:
        check_abi_version(EXPECTED_ABI_VERSION + 1)
    assert exc.value.expected == EXPECTED_ABI_VERSION
    assert exc.value.actual == EXPECTED_ABI_VERSION + 1
    message = str(exc.value)
    assert f"expected {EXPECTED_ABI_VERSION}" in message
    assert f"got {EXPECTED_ABI_VERSION + 1}" in message
