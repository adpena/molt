from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import uuid
from pathlib import Path
from types import ModuleType

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = REPO_ROOT / "tools" / "check_luau_static.py"


def _load_module() -> ModuleType:
    name = f"check_luau_static_{uuid.uuid4().hex}"
    spec = importlib.util.spec_from_file_location(name, MODULE_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(autouse=True)
def _external_volume_env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    ext_root = tmp_path / "ext"
    (ext_root / "cargo-target").mkdir(parents=True, exist_ok=True)
    (ext_root / "molt_cache").mkdir(parents=True, exist_ok=True)
    (ext_root / "diff").mkdir(parents=True, exist_ok=True)
    (ext_root / "tmp").mkdir(parents=True, exist_ok=True)
    (ext_root / "uv-cache").mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(ext_root / "cargo-target"))
    monkeypatch.setenv("MOLT_DIFF_CARGO_TARGET_DIR", str(ext_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(ext_root / "molt_cache"))
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(ext_root / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(ext_root / "tmp"))
    monkeypatch.setenv("UV_CACHE_DIR", str(ext_root / "uv-cache"))
    monkeypatch.setenv("TMPDIR", str(ext_root / "tmp"))


def test_parse_analyzer_warnings_best_effort() -> None:
    mod = _load_module()
    output = "\n".join(
        [
            "fixture.luau:1: warning: UnknownGlobal variable 'x'",
            "fixture.luau:2: warning: LocalShadow local 'i' shadows outer",
            "fixture.luau:3: Warning[TypeMismatch]: expected string, got number",
            "3 warnings generated",
        ]
    )

    warning_count, warning_classes = mod.parse_analyzer_warnings(output)

    assert warning_count == 3
    assert warning_classes == {
        "LocalShadow": 1,
        "TypeMismatch": 1,
        "UnknownGlobal": 1,
    }


def test_require_analyzer_fails_when_missing(tmp_path: Path, monkeypatch) -> None:
    mod = _load_module()
    source = tmp_path / "sample.py"
    source.write_text("print('ok')\n", encoding="utf-8")

    monkeypatch.setattr(mod.shutil, "which", lambda name: None)

    rc = mod.main([str(source), "--require-analyzer"])
    assert rc == 2


def test_batch_runs_and_writes_json_report(tmp_path: Path, monkeypatch) -> None:
    mod = _load_module()

    batch_dir = tmp_path / "corpus"
    batch_dir.mkdir()
    (batch_dir / "a.py").write_text("print('a')\n", encoding="utf-8")
    (batch_dir / "b.py").write_text("print('b')\n", encoding="utf-8")
    (batch_dir / "ignore.txt").write_text("x", encoding="utf-8")

    json_out = tmp_path / "report.json"

    monkeypatch.setattr(mod.shutil, "which", lambda name: "/usr/bin/luau-analyze")

    def _fake_run(cmd, *args, **kwargs):
        if len(cmd) >= 3 and cmd[1] == "-m" and cmd[2] == "molt.cli":
            output_idx = cmd.index("--output") + 1
            Path(cmd[output_idx]).write_text("--!strict\n", encoding="utf-8")
            return subprocess.CompletedProcess(cmd, 0, stdout="build ok", stderr="")

        if cmd[0] == "/usr/bin/luau-analyze":
            stdout = "\n".join(
                [
                    f"{cmd[1]}:1: warning: UnknownGlobal variable 'x'",
                    f"{cmd[1]}:2: Warning[LocalShadow]: local shadows parent",
                ]
            )
            return subprocess.CompletedProcess(cmd, 0, stdout=stdout, stderr="")

        raise AssertionError(f"Unexpected subprocess command: {cmd}")

    monkeypatch.setattr(mod.subprocess, "run", _fake_run)

    rc = mod.main(
        [
            "--batch",
            str(batch_dir),
            "--pattern",
            "*.py",
            "--json-out",
            str(json_out),
        ]
    )

    assert rc == 0
    payload = json.loads(json_out.read_text(encoding="utf-8"))
    assert payload["sources_total"] == 2
    assert payload["transpile_fail"] == 0
    assert payload["analyzer_available"] is True
    assert payload["analyzer_executed"] == 2
    assert payload["warnings_total"] == 4
    assert payload["warning_classes"] == {"LocalShadow": 2, "UnknownGlobal": 2}


def test_missing_analyzer_is_nonfatal_without_requirement(
    tmp_path: Path,
    monkeypatch,
) -> None:
    mod = _load_module()
    source = tmp_path / "single.py"
    source.write_text("print('ok')\n", encoding="utf-8")
    json_out = tmp_path / "single-report.json"

    monkeypatch.setattr(mod.shutil, "which", lambda name: None)

    def _fake_run(cmd, *args, **kwargs):
        if len(cmd) >= 3 and cmd[1] == "-m" and cmd[2] == "molt.cli":
            output_idx = cmd.index("--output") + 1
            Path(cmd[output_idx]).write_text("--!strict\n", encoding="utf-8")
            return subprocess.CompletedProcess(cmd, 0, stdout="build ok", stderr="")
        raise AssertionError(f"Unexpected subprocess command: {cmd}")

    monkeypatch.setattr(mod.subprocess, "run", _fake_run)

    rc = mod.main([str(source), "--json-out", str(json_out)])

    assert rc == 0
    payload = json.loads(json_out.read_text(encoding="utf-8"))
    assert payload["analyzer_available"] is False
    assert payload["analyzer_executed"] == 0
    assert payload["analyzer_skipped"] == 1
    assert payload["warnings_total"] == 0
