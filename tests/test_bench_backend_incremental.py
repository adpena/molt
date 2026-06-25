from __future__ import annotations

import importlib.util
import datetime as dt
import sys
import uuid
from types import SimpleNamespace
from pathlib import Path
from types import ModuleType


REPO_ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = REPO_ROOT / "tools" / "bench_backend_incremental.py"


def _load_module() -> ModuleType:
    name = f"bench_backend_incremental_{uuid.uuid4().hex}"
    spec = importlib.util.spec_from_file_location(name, MODULE_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_case_env_uses_canonical_internal_roots(tmp_path: Path, monkeypatch) -> None:
    mod = _load_module()
    case_root = tmp_path / "case"
    ext_root = tmp_path / "ext"

    env, env_paths = mod._case_env(case_root=case_root, molt_ext_root=ext_root)

    target_root = case_root / "target"
    cache_root = case_root / ".molt_cache"
    diff_root = case_root / "tmp" / "diff"
    tmp_root = case_root / "tmp"
    uv_cache_root = case_root / ".uv-cache"

    assert target_root.is_dir()
    assert cache_root.is_dir()
    assert diff_root.is_dir()
    assert tmp_root.is_dir()
    assert uv_cache_root.is_dir()

    assert env["CARGO_TARGET_DIR"] == str(target_root)
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(target_root)
    assert env["MOLT_CACHE"] == str(cache_root)
    assert env["MOLT_DIFF_ROOT"] == str(diff_root)
    assert env["MOLT_DIFF_TMPDIR"] == str(tmp_root)
    assert env["UV_CACHE_DIR"] == str(uv_cache_root)
    assert env["TMPDIR"] == str(tmp_root)

    assert env_paths == {
        "MOLT_EXT_ROOT": str(ext_root),
        "CARGO_TARGET_DIR": str(target_root),
        "MOLT_DIFF_CARGO_TARGET_DIR": str(target_root),
        "MOLT_CACHE": str(cache_root),
        "MOLT_DIFF_ROOT": str(diff_root),
        "MOLT_DIFF_TMPDIR": str(tmp_root),
        "UV_CACHE_DIR": str(uv_cache_root),
        "TMPDIR": str(tmp_root),
    }


def test_resolve_output_root_defaults_under_tmp(tmp_path: Path, monkeypatch) -> None:
    mod = _load_module()
    ext_root = tmp_path / "ext"
    ext_root.mkdir()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))

    class _FixedDatetime(dt.datetime):
        @classmethod
        def now(cls, tz=None):
            return cls(2026, 4, 9, 10, 11, 12, tzinfo=tz)

    monkeypatch.setattr(mod.dt, "datetime", _FixedDatetime)

    output_root, molt_ext_root = mod._resolve_output_root(None)

    assert (
        output_root == ext_root / "tmp" / "bench_backend_incremental_20260409T101112Z"
    )
    assert molt_ext_root == ext_root


def test_backend_incremental_uses_shared_phase_result_schema() -> None:
    mod = _load_module()
    from tools import throughput_measurement

    assert mod.PhaseResult is throughput_measurement.PhaseResult


def test_run_command_records_shared_measurement_payload(
    tmp_path: Path,
    monkeypatch,
) -> None:
    mod = _load_module()
    output = tmp_path / "program.rs"
    output.write_bytes(b"compiled")

    stdout = "\n".join(f"out-{i}" for i in range(20))
    stderr = "\n".join(f"err-{i}" for i in range(20))

    def fake_limits_from_env(prefix, env):  # type: ignore[no-untyped-def]
        assert prefix == "MOLT_BENCH"
        assert env == {"X": "1"}
        return None

    def fake_guarded_completed_process(command, **kwargs):  # type: ignore[no-untyped-def]
        assert command == ["molt", "build"]
        assert kwargs["cwd"] == tmp_path
        assert kwargs["env"] == {"X": "1"}
        assert kwargs["progress_label"] == "backend-incremental cold"
        return SimpleNamespace(
            stdout=stdout,
            stderr=stderr,
            returncode=0,
            elapsed_s=1.23456,
        )

    monkeypatch.setattr(
        mod.harness_memory_guard, "limits_from_env", fake_limits_from_env
    )
    monkeypatch.setattr(
        mod.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = mod._run_command(
        ["molt", "build"],
        cwd=tmp_path,
        env={"X": "1"},
        timeout_sec=9.0,
        phase="cold",
        output_path=output,
    )

    assert result.phase == "cold"
    assert result.cwd == str(tmp_path)
    assert result.elapsed_sec == 1.235
    assert result.output_size_bytes == len(b"compiled")
    assert result.stdout_tail == "\n".join(f"out-{i}" for i in range(8, 20))
    assert result.stderr_tail == "\n".join(f"err-{i}" for i in range(8, 20))
