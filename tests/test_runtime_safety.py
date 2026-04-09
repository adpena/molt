from __future__ import annotations

from pathlib import Path

import pytest

from tools import runtime_safety


def test_fuzz_workspace_for_runtime_target() -> None:
    workspace = runtime_safety._fuzz_workspace_for_target("string_ops")
    assert workspace == Path("runtime/molt-runtime/fuzz") or workspace == (
        runtime_safety.RUNTIME_FUZZ_DIR
    )


def test_fuzz_workspace_for_root_target() -> None:
    workspace = runtime_safety._fuzz_workspace_for_target("fuzz_ir_parse")
    assert workspace == Path("fuzz") or workspace == runtime_safety.ROOT_FUZZ_DIR


def test_fuzz_workspace_for_unknown_target_raises() -> None:
    with pytest.raises(SystemExit, match="unknown fuzz target"):
        runtime_safety._fuzz_workspace_for_target("definitely_missing_target")


def test_miri_tmp_root_defaults_to_repo_tmp(monkeypatch) -> None:
    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR", "MOLT_EXT_ROOT"):
        monkeypatch.delenv(key, raising=False)

    assert runtime_safety._miri_tmp_root({}) == (
        runtime_safety.ROOT / "tmp" / "runtime_safety" / "miri"
    )


def test_run_miri_preserves_explicit_tmpdir(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, object] = {}

    def fake_run(cmd, env=None, cwd=None, log_path=None):
        captured["cmd"] = cmd
        captured["env"] = env
        captured["cwd"] = cwd
        captured["log_path"] = log_path

    monkeypatch.setenv("TMPDIR", str(tmp_path / "explicit-tmp"))
    monkeypatch.setattr(runtime_safety, "_run", fake_run)

    runtime_safety.run_miri(None)

    assert captured["env"]["TMPDIR"] == str(tmp_path / "explicit-tmp")


def test_run_miri_defaults_to_canonical_tmp_root(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_run(cmd, env=None, cwd=None, log_path=None):
        captured["env"] = env

    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR", "MOLT_EXT_ROOT"):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setattr(runtime_safety, "_run", fake_run)

    runtime_safety.run_miri(None)

    assert captured["env"]["TMPDIR"] == str(
        runtime_safety.ROOT / "tmp" / "runtime_safety" / "miri"
    )
