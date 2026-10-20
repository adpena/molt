from __future__ import annotations

from pathlib import Path
import ntpath
import os
import shutil
import types

import pytest

from molt.default_paths import executable_environment_value
from molt.process_environment import (
    NameRule,
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)

from molt.toolchain_identity import (
    executable_name_candidates,
    executable_search_directories,
)


def test_windows_selection_projects_only_captured_suffix_and_cwd_policy(
    tmp_path: Path,
) -> None:
    path_root = tmp_path / "path"
    current = tmp_path / "cwd"
    environment = ProcessEnvironmentBuilder(
        {
            "Path": str(path_root),
            "PathExt": ".ALT;.EXE",
            "NoDefaultCurrentDirectoryInExePath": "",
        },
        rule=NameRule.WINDOWS,
    )
    assert executable_name_candidates(
        "clang", environment=environment, windows=True
    ) == (
        "clang.ALT",
        "clang.EXE",
    )
    assert executable_name_candidates(
        "clang.ALT", environment=environment, windows=True
    ) == ("clang.ALT", "clang.ALT.ALT", "clang.ALT.EXE")
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


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_executable_environment_uses_canonical_name_and_collision_rules(rule) -> None:
    environment = ProcessEnvironment({"PATH": "first", "Path": "second"}, rule=rule)
    assert executable_environment_value(environment, "PATH") == (
        "first" if rule is NameRule.EXACT else "second"
    )
    assert executable_environment_value(environment, "Path") == "second"
    assert tuple(environment) == (
        ("PATH", "Path") if rule is NameRule.EXACT else ("PATH",)
    )
    nonexpanding = ProcessEnvironment({"ß": "sharp", "SS": "double"}, rule=rule)
    assert executable_environment_value(nonexpanding, "ß") == "sharp"
    assert executable_environment_value(nonexpanding, "SS") == "double"


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_windows_filesystem_policy_does_not_override_environment_name_rule(
    tmp_path, rule
):
    selected = tmp_path / "selected"
    unselected = tmp_path / "unselected"
    environment = ProcessEnvironment(
        {
            "PATH": str(selected),
            "Path": str(unselected),
            "PATHEXT": ".EXE",
            "PathExt": ".ALT",
            "nodefaultcurrentdirectoryinexepath": "1",
        },
        rule=rule,
    )
    assert executable_name_candidates(
        "clang", environment=environment, windows=True
    ) == (("clang.EXE",) if rule is NameRule.EXACT else ("clang.ALT",))
    assert executable_search_directories(
        environment=environment, cwd=tmp_path, windows=True
    ) == ((tmp_path, selected) if rule is NameRule.EXACT else (unselected,))


@pytest.mark.parametrize("command", ["npm", "clang.EXE", "tool.alt", "run.py"])
@pytest.mark.parametrize("pathext", [".ALT;.EXE", ".exe.;.cmd", "", None])
def test_windows_suffix_projection_matches_cpython_which_source(command, pathext):
    # Exercise the running CPython stdlib function with only its OS boundary
    # replaced. Record every candidate in a nonexistent single-directory PATH;
    # no duplicated implementation or hand-written expected candidate table.
    environment = {} if pathext is None else {"PATHEXT": pathext}
    observed = []
    oracle_os = types.SimpleNamespace(
        path=ntpath,
        pathsep=";",
        fsdecode=os.fsdecode,
        fsencode=os.fsencode,
        getenv=environment.get,
        X_OK=os.X_OK,
    )
    oracle = types.FunctionType(
        shutil.which.__code__,
        {
            **shutil.which.__globals__,
            "os": oracle_os,
            "sys": types.SimpleNamespace(platform="win32"),
            "_win_path_needs_curdir": lambda *_args: False,
            "_access_check": lambda candidate, _mode: (
                observed.append(ntpath.basename(candidate)) or False
            ),
        },
        argdefs=shutil.which.__defaults__,
    )
    assert oracle(command, path=r"C:\only") is None
    assert executable_name_candidates(
        command, environment=environment, windows=True
    ) == tuple(observed)
