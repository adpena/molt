from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"


def test_native_tir_skip_pattern_preserves_runtime_behavior(tmp_path: Path) -> None:
    source = tmp_path / "skip_pattern_probe.py"
    source.write_text("print(1)\n", encoding="utf-8")
    binary = tmp_path / "skip_pattern_probe_molt"

    env = {
        **os.environ,
        "PYTHONPATH": str(SRC_DIR),
        "MOLT_EXT_ROOT": str(ROOT),
        "CARGO_TARGET_DIR": os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")),
        "MOLT_DIFF_CARGO_TARGET_DIR": os.environ.get(
            "MOLT_DIFF_CARGO_TARGET_DIR",
            os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")),
        ),
        "MOLT_CACHE": os.environ.get("MOLT_CACHE", str(ROOT / ".molt_cache")),
        "MOLT_DIFF_ROOT": os.environ.get("MOLT_DIFF_ROOT", str(ROOT / "tmp" / "diff")),
        "MOLT_DIFF_TMPDIR": os.environ.get("MOLT_DIFF_TMPDIR", str(ROOT / "tmp")),
        "UV_CACHE_DIR": os.environ.get("UV_CACHE_DIR", str(ROOT / ".uv-cache")),
        "TMPDIR": os.environ.get("TMPDIR", str(ROOT / "tmp")),
        "MOLT_BACKEND_DAEMON": "0",
        "MOLT_TIR_SKIP_PATTERN": "skip_pattern_probe",
        "MOLT_SESSION_ID": f"test-native-tir-skip-pattern-{tmp_path.name}",
        "CARGO_BUILD_JOBS": "1",
    }

    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--build-profile",
            "dev",
            str(source),
            "--out-dir",
            str(tmp_path),
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=300,
    )
    assert build.returncode == 0, build.stderr
    assert binary.exists(), f"expected binary at {binary}"

    run = subprocess.run(
        [str(binary)],
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "1"
