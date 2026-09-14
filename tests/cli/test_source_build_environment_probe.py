"""Recipe identity must not depend on a launcher's imported native modules."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess

import pytest

from molt.cli import source_build_environment as authority
from tests.python_environment_test_support import runtime_identity_manifest


def test_recipe_captures_selected_base_with_isolated_probe(monkeypatch, tmp_path):
    base = tmp_path / "base-python"
    base.write_bytes(b"synthetic; never executed")
    monkeypatch.setattr(authority.sys, "_base_executable", str(base))
    monkeypatch.setenv("PYTHONHOME", "unowned-home")
    monkeypatch.setenv("PYTHONPATH", "unowned-imports")
    expected = runtime_identity_manifest()
    calls = []

    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        return subprocess.CompletedProcess(argv, 0, json.dumps(expected), "")

    monkeypatch.setattr(authority.process_guard, "run_completed_command", run)
    assert authority._python_identity() == expected
    argv, options = calls.pop()
    assert not calls
    assert argv == [
        str(base.resolve()),
        "-I",
        "-S",
        str(Path(authority.python_environment_identity.__file__).resolve()),
        "--capture-runtime",
    ]
    assert options["timeout"] == 120
    assert "PYTHONPATH" not in options["env"]
    assert "PYTHONHOME" not in options["env"]
    assert options["env"]["PYTHONDONTWRITEBYTECODE"] == "1"
    assert options["env"]["PYTHONNOUSERSITE"] == "1"


@pytest.mark.parametrize("output", ["not json", '{"schema":1,"schema":2}', "{}"])
def test_recipe_probe_rejects_invalid_or_unattested_runtime(monkeypatch, output):
    monkeypatch.setattr(
        authority.process_guard,
        "run_completed_command",
        lambda argv, **kwargs: subprocess.CompletedProcess(argv, 0, output, ""),
    )
    with pytest.raises(
        authority.SourceBuildEnvironmentError, match="runtime probe returned"
    ):
        authority._probe_source_build_python(Path("selected-python"))


def test_recipe_probe_reports_failure_instead_of_inprocess_fallback(monkeypatch):
    monkeypatch.setattr(
        authority.process_guard,
        "run_completed_command",
        lambda argv, **kwargs: subprocess.CompletedProcess(
            argv, 7, "", "capture refused"
        ),
    )
    with pytest.raises(
        authority.SourceBuildEnvironmentError, match="runtime content: capture refused"
    ):
        authority._probe_source_build_python(Path("selected-python"))
