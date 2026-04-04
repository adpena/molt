from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"


def _compile_and_run(source: str, profile: str) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        src_path = tmp_path / "loop_join_semantics.py"
        src_path.write_text(source)
        binary_path = tmp_path / "loop_join_semantics_molt"

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
            "MOLT_SESSION_ID": f"test-loop-join-{profile}",
        }

        build = subprocess.run(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                "--build-profile",
                profile,
                str(src_path),
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
        assert binary_path.exists(), f"expected binary at {binary_path}"

        run = subprocess.run(
            [str(binary_path)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        assert run.returncode == 0, run.stderr
        return run.stdout.strip()


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_loop_join_semantics_match_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        def f():
            acc = 0
            i = 0
            while i < 3:
                j = 0
                while j < 4:
                    if j < 2:
                        picked = i + j
                    else:
                        picked = j + 1
                    acc = acc + picked
                    j = j + 1
                i = i + 1
            print(acc)

        f()
        """
    )
    expected = subprocess.run(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile) == expected
