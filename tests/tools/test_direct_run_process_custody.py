from __future__ import annotations

import argparse
import ast
import importlib.util
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[2]


def _load_safe_run():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_safe_run", ROOT / "tools" / "safe_run.py"
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_safe_run_delegates_process_custody_to_memory_guard(tmp_path: Path) -> None:
    safe_run = _load_safe_run()
    ns = argparse.Namespace(rss_mb=512, timeout=7.5, poll=0.25)
    summary_path = tmp_path / "summary.json"

    command = safe_run._guard_command(
        ns,
        [sys.executable, "-c", "print('ok')"],
        summary_path,
    )

    assert command[:2] == [sys.executable, str(ROOT / "tools" / "memory_guard.py")]
    assert "--max-rss-gb" in command
    assert "--max-total-rss-gb" in command
    assert "--summary-json" in command
    assert str(summary_path) in command
    assert command[-4] == "--"
    assert command[-3:] == [sys.executable, "-c", "print('ok')"]


def test_direct_run_tools_have_no_parallel_kill_authority() -> None:
    for relative in ("tools/safe_run.py", "tools/compile_progress.py"):
        source = (ROOT / relative).read_text(encoding="utf-8")
        module = ast.parse(source)
        imported_modules = {
            alias.name
            for node in module.body
            if isinstance(node, ast.Import)
            for alias in node.names
        }
        process_kill_calls = [
            node
            for node in ast.walk(module)
            if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "os"
            and node.func.attr in {"kill", "killpg"}
        ]

        assert "signal" not in imported_modules
        assert "killpg" not in source
        assert "_kill_run_scoped_processes" not in source
        assert not process_kill_calls
