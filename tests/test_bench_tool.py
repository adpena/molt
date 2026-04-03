from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import textwrap
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
BENCH_TOOL_PATH = REPO_ROOT / "tools" / "bench.py"
BENCH_TOOL_SPEC = importlib.util.spec_from_file_location(
    "bench_tool_under_test", BENCH_TOOL_PATH
)
assert BENCH_TOOL_SPEC is not None and BENCH_TOOL_SPEC.loader is not None
bench_tool = importlib.util.module_from_spec(BENCH_TOOL_SPEC)
BENCH_TOOL_SPEC.loader.exec_module(bench_tool)


def _run_bench(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["python3", "tools/bench.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_bench_no_cpython_sets_null_baseline(tmp_path: Path) -> None:
    script = tmp_path / "fast_script.py"
    script.write_text("print(1)\n", encoding="utf-8")
    out_json = tmp_path / "bench.json"

    res = _run_bench(
        "--no-cpython",
        "--no-pypy",
        "--no-codon",
        "--no-nuitka",
        "--no-pyodide",
        "--samples",
        "1",
        "--warmup",
        "0",
        "--json-out",
        str(out_json),
        "--script",
        str(script),
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads(out_json.read_text(encoding="utf-8"))
    entry = payload["benchmarks"][script.name]
    assert entry["cpython_time_s"] is None
    assert entry["molt_ok"] is True
    assert entry["molt_speedup"] is None


def test_bench_runtime_timeout_marks_molt_not_ok(tmp_path: Path) -> None:
    script = tmp_path / "slow_script.py"
    script.write_text(
        textwrap.dedent(
            """
            import time

            time.sleep(2.0)
            print("done")
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )
    out_json = tmp_path / "bench_timeout.json"

    res = _run_bench(
        "--no-cpython",
        "--no-pypy",
        "--no-codon",
        "--no-nuitka",
        "--no-pyodide",
        "--samples",
        "1",
        "--warmup",
        "0",
        "--runtime-timeout-sec",
        "0.1",
        "--json-out",
        str(out_json),
        "--script",
        str(script),
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads(out_json.read_text(encoding="utf-8"))
    entry = payload["benchmarks"][script.name]
    assert entry["molt_ok"] is False
    assert entry["molt_time_s"] is None


def test_molt_build_cmd_supports_explicit_profile() -> None:
    assert bench_tool._molt_build_cmd("release") == [
        "uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "-m",
        "molt.cli",
        "build",
        "--build-profile",
        "release",
    ]


def test_canonical_bench_env_uses_repo_roots_and_preserves_session() -> None:
    env = bench_tool._canonical_bench_env({"MOLT_SESSION_ID": "bench-review"})

    assert env["MOLT_EXT_ROOT"] == str(bench_tool.REPO_ROOT)
    assert env["CARGO_TARGET_DIR"] == str(bench_tool.REPO_ROOT / "target")
    assert env["MOLT_CACHE"] == str(bench_tool.REPO_ROOT / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(bench_tool.REPO_ROOT / "tmp" / "diff")
    assert env["TMPDIR"] == str(bench_tool.REPO_ROOT / "tmp")
    assert env["PYTHONPATH"] == str(bench_tool.REPO_ROOT / "src")
    assert env["MOLT_SESSION_ID"] == "bench-review"


def test_bench_defaults_baseline_to_canonical_results_path() -> None:
    assert bench_tool.DEFAULT_BASELINE_PATH == (
        bench_tool.REPO_ROOT / "bench" / "results" / "baseline.json"
    )


def test_bench_cli_passes_molt_profile(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, object] = {}

    monkeypatch.setattr(bench_tool, "_enable_line_buffering", lambda: None)
    monkeypatch.setattr(bench_tool, "_prune_backend_daemons", lambda: None)
    monkeypatch.setattr(
        bench_tool,
        "bench_results",
        lambda *args, **kwargs: (
            captured.update({"molt_profile": args[10], "benchmarks": args[0]}) or {}
        ),
    )
    monkeypatch.setattr(bench_tool, "_git_rev", lambda: "deadbeef")
    monkeypatch.setattr(bench_tool, "write_json", lambda path, payload: None)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "tools/bench.py",
            "--molt-profile",
            "release",
            "--script",
            str(tmp_path / "bench_sample.py"),
        ],
    )
    (tmp_path / "bench_sample.py").write_text("print(1)\n", encoding="utf-8")

    bench_tool.main()

    assert captured["molt_profile"] == "release"
    assert captured["benchmarks"] == [str(tmp_path / "bench_sample.py")]


def test_bench_cli_defaults_molt_profile_to_release(
    monkeypatch, tmp_path: Path
) -> None:
    captured: dict[str, object] = {}

    monkeypatch.setattr(bench_tool, "_enable_line_buffering", lambda: None)
    monkeypatch.setattr(bench_tool, "_prune_backend_daemons", lambda: None)
    monkeypatch.setattr(
        bench_tool,
        "bench_results",
        lambda *args, **kwargs: captured.update({"molt_profile": args[10]}) or {},
    )
    monkeypatch.setattr(bench_tool, "_git_rev", lambda: "deadbeef")
    monkeypatch.setattr(bench_tool, "write_json", lambda path, payload: None)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "tools/bench.py",
            "--script",
            str(tmp_path / "bench_sample.py"),
        ],
    )
    (tmp_path / "bench_sample.py").write_text("print(1)\n", encoding="utf-8")

    bench_tool.main()

    assert captured["molt_profile"] == "release"
