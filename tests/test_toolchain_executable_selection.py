from __future__ import annotations

from pathlib import Path

import pytest

from molt.toolchain_identity import (
    executable_environment_value,
    executable_name_candidates,
    executable_search_directories,
)


def test_windows_selection_projects_only_captured_suffix_and_cwd_policy(
    tmp_path: Path,
) -> None:
    path_root = tmp_path / "path"
    current = tmp_path / "cwd"
    environment = {
        "Path": str(path_root),
        "PathExt": ".ALT;.EXE",
        "NoDefaultCurrentDirectoryInExePath": "",
    }
    assert executable_name_candidates(
        "clang", environment=environment, windows=True
    ) == (
        "clang",
        "clang.ALT",
        "clang.EXE",
    )
    assert executable_name_candidates(
        "clang.ALT", environment=environment, windows=True
    ) == ("clang.ALT",)
    assert executable_search_directories(
        environment=environment, cwd=current, windows=True
    ) == (path_root,)
    environment.pop("NoDefaultCurrentDirectoryInExePath")
    assert executable_search_directories(
        environment=environment, cwd=current, windows=True
    ) == (current, path_root)


@pytest.mark.parametrize("windows", [False, True])
def test_absent_captured_path_disables_implicit_search(
    tmp_path: Path, windows: bool
) -> None:
    assert (
        executable_search_directories(environment={}, cwd=tmp_path, windows=windows)
        == ()
    )
    assert (
        executable_search_directories(
            environment={"PATH": ""}, cwd=tmp_path, windows=windows
        )
        == ()
    )


def test_posix_path_preserves_literal_spaces_and_explicit_empty_component(
    tmp_path: Path,
) -> None:
    assert executable_search_directories(
        environment={"PATH": " spaced :"},
        cwd=tmp_path,
        windows=False,
    ) == (tmp_path / " spaced ", tmp_path)


def test_windows_conflicting_search_key_spellings_fail_closed() -> None:
    with pytest.raises(ValueError, match="conflicting captured environment"):
        executable_environment_value(
            {"Path": "first", "PATH": "second"}, "PATH", windows=True
        )
