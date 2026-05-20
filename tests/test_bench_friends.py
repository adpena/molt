from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import textwrap
from pathlib import Path

from tests.native_process_guard import run_native_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
TOOL_PATH = REPO_ROOT / "tools" / "bench_friends.py"


def _load_tool_module():
    spec = importlib.util.spec_from_file_location("bench_friends_under_test", TOOL_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _run_tool(*args: str) -> subprocess.CompletedProcess[str]:
    return run_native_test_process(
        ["python3", "tools/bench_friends.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_default_output_root_is_canonical_bench_results(monkeypatch) -> None:
    module = _load_tool_module()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(REPO_ROOT))

    output_root = module._default_output_root()

    assert output_root.parent == REPO_ROOT / "bench" / "results" / "friends"


def test_bench_friends_local_suite_runs(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)

    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "local_smoke"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "runs_unmodified"
            repeat = 2
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.01)"]

            [suite.runners.molt]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.02)"]

            [suite.runners.friend]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.03)"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr

    results_json = output_root / "results.json"
    assert results_json.exists()
    payload = json.loads(results_json.read_text(encoding="utf-8"))
    assert payload["suites"]
    suite = payload["suites"][0]
    assert suite["id"] == "local_smoke"
    assert suite["status"] == "ok"
    assert suite["metrics"]["molt_vs_cpython_speedup"] is not None

    summary = output_root / "summary.md"
    assert summary.exists()
    summary_text = summary.read_text(encoding="utf-8")
    assert "local_smoke" in summary_text


def test_bench_friends_include_disabled_with_dry_run(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "dry_out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "disabled_suite"
            enabled = false
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "requires_adapter"

            [suite.runners.cpython]
            skip_reason = "not configured"
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--include-disabled",
        "--dry-run",
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    assert payload["suites"][0]["status"] == "skipped"


def test_bench_friends_nuitka_pyodide_runners(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_ext"
    suite_root = tmp_path / "suite_ext"
    suite_root.mkdir(parents=True, exist_ok=True)

    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "ext_runners_smoke"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "requires_adapter"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.01)"]

            [suite.runners.molt]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.02)"]

            [suite.runners.nuitka]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.03)"]

            [suite.runners.pyodide]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.04)"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "ok"
    assert suite["runners"]["nuitka"]["status"] == "ok"
    assert suite["runners"]["pyodide"]["status"] == "ok"
    assert suite["metrics"]["nuitka_median_s"] is not None
    assert suite["metrics"]["pyodide_median_s"] is not None
    assert suite["metrics"]["molt_vs_nuitka_speedup"] is not None
    assert suite["metrics"]["molt_vs_pyodide_speedup"] is not None

    summary_text = (output_root / "summary.md").read_text(encoding="utf-8")
    assert "Nuitka s" in summary_text
    assert "Pyodide s" in summary_text
