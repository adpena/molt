from __future__ import annotations

import json
import subprocess
import textwrap
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


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
        "--no-cython",
        "--no-numba",
        "--no-codon",
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
        "--no-cython",
        "--no-numba",
        "--no-codon",
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
    assert entry["molt_time_s"] == 0.0
