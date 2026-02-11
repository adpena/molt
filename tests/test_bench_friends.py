from __future__ import annotations

import json
import subprocess
import textwrap
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


def _run_tool(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["python3", "tools/bench_friends.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


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
