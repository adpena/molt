from __future__ import annotations

import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
DX_BUILD_TIMER = REPO_ROOT / "tools" / "dx_build_timer.py"


def _load_dx_build_timer():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_dx_build_timer",
        DX_BUILD_TIMER,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_run_uses_shared_memory_guard(monkeypatch, tmp_path: Path) -> None:
    module = _load_dx_build_timer()
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return SimpleNamespace(
            returncode=0, stdout="ok\n", stderr="err\n", elapsed_s=0.125
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc, elapsed, tail = module._run(
        ["cargo", "build"],
        {"CARGO_TARGET_DIR": str(tmp_path / "target")},
        tmp_path,
    )

    assert rc == 0
    assert elapsed == 0.125
    assert tail == "err"
    assert calls == [
        {
            "cmd": ["cargo", "build"],
            "cwd": tmp_path,
            "env": {"CARGO_TARGET_DIR": str(tmp_path / "target")},
            "capture_output": True,
            "text": True,
            "prefix": "MOLT_DX_BUILD",
            "progress_label": None,
        }
    ]


def test_touch_journal_restores_and_recovers_crash_left_marker(tmp_path: Path) -> None:
    module = _load_dx_build_timer()
    source = tmp_path / "value_range.rs"
    original = b"fn value_range() {}\n"
    source.write_bytes(original)
    journal = module.TouchJournal(tmp_path / "target" / ".dx_build_timer_touches.json")

    entry = journal.touch(source)
    assert source.read_bytes() == original + module.TOUCH_MARKER
    journal.restore(entry)
    assert source.read_bytes() == original
    assert not journal.path.exists()

    journal.touch(source)
    assert source.read_bytes() == original + module.TOUCH_MARKER
    module.TouchJournal(journal.path).recover()
    assert source.read_bytes() == original
    assert not journal.path.exists()


def test_touch_journal_refuses_to_overwrite_external_edit(tmp_path: Path) -> None:
    module = _load_dx_build_timer()
    source = tmp_path / "function_compiler.rs"
    source.write_bytes(b"fn before() {}\n")
    journal = module.TouchJournal(tmp_path / "target" / ".dx_build_timer_touches.json")

    entry = journal.touch(source)
    source.write_bytes(b"fn edited_elsewhere() {}\n")

    with pytest.raises(RuntimeError, match="content changed outside dx_build_timer"):
        journal.restore(entry)
    assert json.loads(journal.path.read_text(encoding="utf-8"))["entries"] == [entry]


def test_write_snapshot_records_active_command(tmp_path: Path) -> None:
    module = _load_dx_build_timer()
    out = tmp_path / "timer.json"
    args = SimpleNamespace(
        profile="release-fast",
        package="molt-backend",
        features="native-backend",
        runs=2,
        target_dir=str(tmp_path / "target"),
        json_out=str(out),
    )

    module._write_snapshot(
        args,
        {"inc-value_range": {"samples_sec": [1.25], "rc": 0}},
        cargo_version="cargo 1.96.0",
        prime={"elapsed_sec": 0.5, "rc": 0, "cmd": ["cargo", "build"]},
        active={"label": "test-lib", "run": 1, "cmd": ["cargo", "test"]},
    )

    payload = json.loads(out.read_text(encoding="utf-8"))
    assert payload["meta"]["target_dir"] == str(tmp_path / "target")
    assert payload["prime"]["elapsed_sec"] == 0.5
    assert payload["active"] == {
        "label": "test-lib",
        "run": 1,
        "cmd": ["cargo", "test"],
    }
    assert payload["results"]["inc-value_range"]["samples_sec"] == [1.25]
