from __future__ import annotations

from contextlib import contextmanager
import json
import sys
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))

import translation_validate


def test_temp_root_defaults_to_repo_tmp(monkeypatch) -> None:
    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR", "MOLT_EXT_ROOT"):
        monkeypatch.delenv(key, raising=False)

    assert (
        translation_validate._temp_root({}) == translation_validate._REPO_ROOT / "tmp"
    )


def test_temp_root_prefers_explicit_overrides(tmp_path: Path) -> None:
    diff_tmp = tmp_path / "diff-tmp"
    ambient_tmp = tmp_path / "ambient-tmp"
    ext_root = tmp_path / "ext-root"
    env = {
        "MOLT_DIFF_TMPDIR": str(diff_tmp),
        "TMPDIR": str(ambient_tmp),
        "MOLT_EXT_ROOT": str(ext_root),
    }

    assert translation_validate._temp_root(env) == diff_tmp

    env.pop("MOLT_DIFF_TMPDIR")
    assert translation_validate._temp_root(env) == ambient_tmp

    env.pop("TMPDIR")
    assert translation_validate._temp_root(env) == ext_root / "tmp"


def test_target_root_defaults_to_repo_target(monkeypatch) -> None:
    for key in ("CARGO_TARGET_DIR", "MOLT_EXT_ROOT"):
        monkeypatch.delenv(key, raising=False)

    assert translation_validate._cargo_target_root({}) == (
        translation_validate._REPO_ROOT / "target"
    )


def test_target_root_prefers_explicit_override(tmp_path: Path) -> None:
    ext_root = tmp_path / "ext-root"
    cargo_target_dir = tmp_path / "cargo-target"
    env = {
        "CARGO_TARGET_DIR": str(cargo_target_dir),
        "MOLT_EXT_ROOT": str(ext_root),
    }

    assert translation_validate._cargo_target_root(env) == cargo_target_dir

    env.pop("CARGO_TARGET_DIR")
    assert translation_validate._cargo_target_root(env) == ext_root / "target"


def test_resolve_target_python_uses_project_floor(tmp_path: Path) -> None:
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "sample"\nrequires-python = ">=3.13,<3.15"\n',
        encoding="utf-8",
    )

    target = translation_validate._resolve_target_python(
        None,
        project_root=tmp_path,
    )

    assert target.short == "3.13"


def test_python_command_candidates_are_versioned_not_process_python(
    monkeypatch,
) -> None:
    monkeypatch.setattr(translation_validate.shutil, "which", lambda name: None)
    target = translation_validate.molt_cli._SUPPORTED_TARGET_PYTHON_BY_SHORT["3.14"]

    candidates = translation_validate._target_python_command_candidates(
        target,
        override=None,
    )

    assert [sys.executable] not in candidates
    assert ["python3.14"] in candidates


def test_target_python_command_prefers_uv_python_find_path(monkeypatch) -> None:
    target = translation_validate.molt_cli._SUPPORTED_TARGET_PYTHON_BY_SHORT["3.13"]
    translation_validate._target_python_command_cached.cache_clear()

    monkeypatch.delenv("MOLT_TV_PYTHON", raising=False)
    monkeypatch.setattr(translation_validate.shutil, "which", lambda name: "uv.exe")

    def fake_run_subprocess(cmd, **kwargs):
        if cmd[:3] == ["uv.exe", "python", "find"]:
            return "C:/py313/python.exe\n", "", 0
        if cmd[:2] == ["C:/py313/python.exe", "-c"]:
            return "3.13", "", 0
        raise AssertionError(f"unexpected command: {cmd}")

    monkeypatch.setattr(translation_validate, "_run_subprocess", fake_run_subprocess)

    assert translation_validate._target_python_command(target) == [
        "C:/py313/python.exe"
    ]


def test_run_cpython_uses_verified_target_python_command(monkeypatch) -> None:
    target = translation_validate.molt_cli._SUPPORTED_TARGET_PYTHON_BY_SHORT["3.13"]
    calls: list[list[str]] = []

    monkeypatch.setattr(
        translation_validate,
        "_target_python_command",
        lambda target_python: [f"python{target_python.short}"],
    )

    def fake_run_subprocess(cmd, **kwargs):
        calls.append(cmd)
        return "ok\n", "", 0

    monkeypatch.setattr(translation_validate, "_run_subprocess", fake_run_subprocess)

    result = translation_validate._run_cpython(
        "case.py",
        timeout=5,
        target_python=target,
    )

    assert result.ok
    assert calls == [["python3.13", "case.py"]]


def test_run_molt_build_stamps_target_python(
    monkeypatch,
    tmp_path: Path,
) -> None:
    target = translation_validate.molt_cli._SUPPORTED_TARGET_PYTHON_BY_SHORT["3.14"]
    source = tmp_path / "case.py"
    source.write_text("print('ok')\n", encoding="utf-8")
    build_cmds: list[list[str]] = []

    monkeypatch.setattr(
        translation_validate,
        "_target_python_command",
        lambda target_python: [f"python{target_python.short}"],
    )
    monkeypatch.setattr(translation_validate, "_temp_root", lambda env=None: tmp_path)

    def fake_run_subprocess(cmd, **kwargs):
        if "-m" in cmd and "molt.cli" in cmd:
            build_cmds.append(cmd)
            output = Path(cmd[cmd.index("--output") + 1])
            output.write_text("#!/bin/sh\n", encoding="utf-8")
            return "", "", 0
        return "ok\n", "", 0

    monkeypatch.setattr(translation_validate, "_run_subprocess", fake_run_subprocess)

    result = translation_validate._run_molt(
        str(source),
        timeout=5,
        build_profile="dev",
        target_python=target,
    )

    assert result.ok
    assert build_cmds
    build_cmd = build_cmds[0]
    assert build_cmd[:4] == ["python3.14", "-m", "molt.cli", "build"]
    assert "--python-version" in build_cmd
    assert build_cmd[build_cmd.index("--python-version") + 1] == "3.14"


def test_main_wraps_run_in_conformance_memory_guard_scope(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    src = tmp_path / "case.py"
    src.write_text("print('ok')\n", encoding="utf-8")
    calls: list[dict[str, object]] = []
    validate_calls: list[dict[str, object]] = []

    @contextmanager
    def fake_guarded_harness_scope(**kwargs):
        calls.append(kwargs)

        class Scope:
            limits = kwargs["limits"]
            memory_guard = {"enabled": True, "sentinel": "unit"}

        yield Scope()

    monkeypatch.setattr(
        translation_validate.harness_memory_guard,
        "guarded_harness_scope",
        fake_guarded_harness_scope,
    )
    monkeypatch.setattr(
        translation_validate,
        "validate_file",
        lambda *args, **kwargs: (
            validate_calls.append(kwargs)
            or translation_validate.ValidationResult(
                source_path=str(src),
                cpython=translation_validate.RunResult("ok\n", "", 0, 1.0, "cpython"),
                molt=translation_validate.RunResult("ok\n", "", 0, 1.0, "molt"),
                match_molt_vs_cpython=True,
            )
        ),
    )

    rc = translation_validate.main(["--json", "--python-version", "3.14", str(src)])

    assert rc == 0
    assert calls[0]["prefix"] == "MOLT_CONFORMANCE"
    assert calls[0]["repo_root"] == translation_validate._REPO_ROOT
    assert calls[0]["label"] == "translation_validate"
    assert validate_calls[0]["target_python"].short == "3.14"
    payload = json.loads(capsys.readouterr().out)
    assert payload["memory_guard"] == {"enabled": True, "sentinel": "unit"}
