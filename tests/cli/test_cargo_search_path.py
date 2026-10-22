"""The PATH a Molt-driven Cargo build runs under is derived, never inherited (HF-385)."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from molt.cli import cargo_search_path as search

pytestmark = pytest.mark.skipif(
    not search.derives_cargo_search_path(),
    reason="Windows keeps the caller's PATH (unverified cell)",
)

_STANDARD = os.confstr("CS_PATH").split(os.pathsep)


def test_standard_utility_directories_are_posix_cs_path() -> None:
    assert [os.fspath(path) for path in search.standard_utility_directories()] == [
        entry for entry in _STANDARD if entry
    ]


def test_path_lists_llvm_then_tool_directories_then_standard_utilities() -> None:
    value = search.cargo_search_path(
        {"MOLT_LLVM_PREFIX": "/opt/llvm-22", "PATH": "/caller/bin"},
        (
            Path("/toolchains/rust/bin/rustc"),
            Path("/toolchains/rust/bin/cargo"),
            Path("/usr/bin/cc"),
            Path("/caches/sccache/bin/sccache"),
        ),
    )
    expected = ["/opt/llvm-22/bin", "/toolchains/rust/bin", "/usr/bin"]
    expected.append("/caches/sccache/bin")
    expected.extend(entry for entry in _STANDARD if entry and entry not in expected)
    assert value.split(os.pathsep) == expected


def test_caller_path_never_reaches_cargo() -> None:
    tools = (Path("/toolchains/rust/bin/rustc"), Path("/usr/bin/cc"))
    first = search.cargo_search_path({"PATH": "/a:/b"}, tools)
    second = search.cargo_search_path({"PATH": "/home/user/.bun/bin:/opt/x/bin"}, tools)
    unset = search.cargo_search_path({}, tools)
    assert first == second == unset
    assert "/a" not in first.split(os.pathsep)
    assert "/home/user/.bun/bin" not in second.split(os.pathsep)


def test_without_llvm_selection_no_llvm_directory_appears() -> None:
    value = search.cargo_search_path({"MOLT_LLVM_PREFIX": "  "}, ())
    assert value.split(os.pathsep) == [entry for entry in _STANDARD if entry]


@pytest.mark.parametrize(
    ("environment", "executables", "message"),
    [
        ({"MOLT_LLVM_PREFIX": "relative/llvm"}, (), "must be absolute"),
        ({}, (Path("bin/rustc"),), "not absolute"),
        ({}, (Path(f"/bad{os.pathsep}dir/rustc"),), "contains"),
    ],
)
def test_ambiguous_inputs_fail_closed(environment, executables, message) -> None:
    with pytest.raises(ValueError, match=message):
        search.cargo_search_path(environment, executables)
