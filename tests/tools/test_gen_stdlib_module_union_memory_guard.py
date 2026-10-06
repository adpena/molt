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


def _fake_capture(version: str):
    return (
        ("abc", "sys"),
        ("asyncio",),
        ("abc", "asyncio.base_events"),
        ("asyncio",),
    )


def test_generated_outputs_query_the_target_python_authority(
    monkeypatch, tmp_path: Path
) -> None:
    output = tmp_path / "stdlib_module_union.py"
    monkeypatch.setattr(gen_stdlib_module_union, "OUT_PATH", output)
    queried: list[str] = []

    def fake_capture(version: str):
        queried.append(version)
        return _fake_capture(version)

    monkeypatch.setattr(gen_stdlib_module_union, "_capture_version", fake_capture)

    rendered = gen_stdlib_module_union.generated_outputs()

    versions = gen_stdlib_module_union.DEFAULT_PYTHONS
    assert tuple(queried) == tuple(versions)
    assert set(rendered) == {
        output,
        *(
            tmp_path / f"stdlib_module_union_{version.replace('.', '_')}.py"
            for version in versions
        ),
    }
    assert not any(path.exists() for path in rendered)


def test_generated_outputs_fail_closed_on_a_retired_version_module(
    monkeypatch, tmp_path: Path
) -> None:
    output = tmp_path / "stdlib_module_union.py"
    monkeypatch.setattr(gen_stdlib_module_union, "OUT_PATH", output)
    retired = tmp_path / "stdlib_module_union_3_11.py"
    retired.write_text(
        f'"""\n{gen_stdlib_module_union.GENERATED_DATA_MARKER}\n"""\n',
        encoding="utf-8",
    )
    hand_written = tmp_path / "stdlib_module_union_notes.py"
    hand_written.write_text("# not generated\n", encoding="utf-8")

    def unreachable(version: str):
        raise AssertionError("retired outputs must fail before querying Python")

    monkeypatch.setattr(gen_stdlib_module_union, "_capture_version", unreachable)

    with pytest.raises(RuntimeError, match="no longer renders") as raised:
        gen_stdlib_module_union.generated_outputs()

    assert str(retired) in str(raised.value)
    assert str(hand_written) not in str(raised.value)
    assert retired.exists()
