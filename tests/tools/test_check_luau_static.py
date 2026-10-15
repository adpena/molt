from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest
import shutil
from tests.process_guard_common import install_module_view
from tools import check_luau_static as mod


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
    source = tmp_path / "sample.py"
    source.write_text("print('ok')\n", encoding="utf-8")

    install_module_view(monkeypatch, "shutil", shutil, mod, which=lambda name: None)

    rc = mod.main([str(source), "--require-analyzer"])
    assert rc == 2


def test_batch_runs_and_writes_json_report(tmp_path: Path, monkeypatch) -> None:
    batch_dir = tmp_path / "corpus"
    batch_dir.mkdir()
    (batch_dir / "a.py").write_text("print('a')\n", encoding="utf-8")
    (batch_dir / "b.py").write_text("print('b')\n", encoding="utf-8")
    (batch_dir / "ignore.txt").write_text("x", encoding="utf-8")

    json_out = tmp_path / "report.json"

    install_module_view(
        monkeypatch, "shutil", shutil, mod, which=lambda name: "/usr/bin/luau-analyze"
    )

    def _fake_run(cmd, **kwargs):
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

    monkeypatch.setattr(
        mod.harness_memory_guard, "guarded_completed_process", _fake_run
    )

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
    source = tmp_path / "single.py"
    source.write_text("print('ok')\n", encoding="utf-8")
    json_out = tmp_path / "single-report.json"

    install_module_view(monkeypatch, "shutil", shutil, mod, which=lambda name: None)

    def _fake_run(cmd, **kwargs):
        if len(cmd) >= 3 and cmd[1] == "-m" and cmd[2] == "molt.cli":
            output_idx = cmd.index("--output") + 1
            Path(cmd[output_idx]).write_text("--!strict\n", encoding="utf-8")
            return subprocess.CompletedProcess(cmd, 0, stdout="build ok", stderr="")
        raise AssertionError(f"Unexpected subprocess command: {cmd}")

    monkeypatch.setattr(
        mod.harness_memory_guard, "guarded_completed_process", _fake_run
    )

    rc = mod.main([str(source), "--json-out", str(json_out)])

    assert rc == 0
    payload = json.loads(json_out.read_text(encoding="utf-8"))
    assert payload["analyzer_available"] is False
    assert payload["analyzer_executed"] == 0
    assert payload["analyzer_skipped"] == 1
    assert payload["warnings_total"] == 0
