from __future__ import annotations

import pytest

from molt.cli.target_python import TargetPythonVersion, require_verified_target_python


def test_verified_windows_312_tuple_resolves() -> None:
    target = TargetPythonVersion(3, 12, 0)
    assert require_verified_target_python(target, platform="windows") is target


def test_unverified_tuple_fails_with_matrix_diagnostic() -> None:
    with pytest.raises(
        ValueError,
        match=r"CPython 3\.13 on windows.*CPython 3\.12 on windows",
    ):
        require_verified_target_python(
            TargetPythonVersion(3, 13, 0), platform="windows"
        )
