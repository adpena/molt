from __future__ import annotations

import json
import runpy
import subprocess
from pathlib import Path

import pytest

from tools import gen_stdlib_module_union


def _stdlib_payload() -> str:
    return json.dumps(
        {
            "modules": ["abc", "sys"],
            "packages": ["asyncio"],
            "py_modules": ["abc", "asyncio.base_events"],
            "py_packages": ["asyncio"],
        }
    )


def test_capture_version_uses_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout=_stdlib_payload(), stderr="")

    monkeypatch.setattr(
        gen_stdlib_module_union.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    modules, packages, py_modules, py_packages = (
        gen_stdlib_module_union._capture_version("3.12")
    )

    assert modules == ("abc", "sys")
    assert packages == ("asyncio",)
    assert py_modules == ("abc", "asyncio.base_events")
    assert py_packages == ("asyncio",)
    assert captured["cmd"][:5] == ["uv", "run", "--no-project", "--python", "3.12"]
    assert captured["kwargs"]["prefix"] == "MOLT_TEST_SUITE"
    assert captured["kwargs"]["cwd"] == gen_stdlib_module_union.ROOT
    assert captured["kwargs"]["capture_output"] is True


def test_capture_version_preserves_check_output_failure(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):
        return subprocess.CompletedProcess(cmd, 9, stdout="partial", stderr="oom")

    monkeypatch.setattr(
        gen_stdlib_module_union.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    with pytest.raises(subprocess.CalledProcessError) as exc_info:
        gen_stdlib_module_union._capture_version("3.14")

    assert exc_info.value.returncode == 9
    assert exc_info.value.output == "partial"
    assert exc_info.value.stderr == "oom"


def test_render_outputs_split_version_data_and_preserve_facade(tmp_path: Path) -> None:
    output = tmp_path / "stdlib_module_union.py"
    rendered = gen_stdlib_module_union.render_outputs(
        output=output,
        versions=("3.12", "3.13"),
        by_version_modules={
            "3.12": ("abc", "sys"),
            "3.13": ("abc", "annotationlib"),
        },
        by_version_packages={
            "3.12": ("asyncio",),
            "3.13": ("asyncio", "pathlib"),
        },
        by_version_py_modules={
            "3.12": ("abc", "asyncio.base_events"),
            "3.13": ("abc", "pathlib._local"),
        },
        by_version_py_packages={
            "3.12": ("asyncio",),
            "3.13": ("asyncio", "pathlib"),
        },
    )

    for path, text in rendered.items():
        path.write_text(text, encoding="utf-8")

    namespace = runpy.run_path(str(output))

    assert namespace["BASELINE_PYTHON_VERSIONS"] == ("3.12", "3.13")
    assert namespace["STDLIB_MODULES_BY_VERSION"] == {
        "3.12": ("abc", "sys"),
        "3.13": ("abc", "annotationlib"),
    }
    assert namespace["STDLIB_MODULE_UNION"] == ("abc", "annotationlib", "sys")
    assert namespace["STDLIB_PACKAGE_UNION"] == ("asyncio", "pathlib")
    assert namespace["STDLIB_PY_SUBMODULE_UNION"] == (
        "asyncio.base_events",
        "pathlib._local",
    )
    assert (tmp_path / "stdlib_module_union_3_12.py").exists()
    assert (tmp_path / "stdlib_module_union_3_13.py").exists()
    facade = output.read_text(encoding="utf-8")
    assert "_load_version_data" in facade
    assert "STDLIB_MODULES = (" not in facade
