from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]


def _alternate_samefile_root(root: Path) -> Path | None:
    text = str(root)
    candidates: list[Path] = []
    if "/Projects/" in text:
        candidates.append(Path(text.replace("/Projects/", "/projects/")))
    if "/projects/" in text:
        candidates.append(Path(text.replace("/projects/", "/Projects/")))
    for candidate in candidates:
        if candidate == root or not candidate.exists():
            continue
        try:
            if candidate.samefile(root):
                return candidate
        except OSError:
            continue
    return None


def test_native_build_survives_samefile_root_spelling_mismatch(
    tmp_path: Path,
) -> None:
    alt_root = _alternate_samefile_root(ROOT)
    if alt_root is None:
        pytest.skip("samefile alternate repo root spelling is unavailable")

    src_path = tmp_path / "import_os.py"
    out_path = tmp_path / "import_os"
    cache_dir = alt_root / "tmp" / f"root-normalization-cache-{tmp_path.name}"
    src_path.write_text("import os\nprint('ok')\n", encoding="utf-8")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(alt_root / "src")
    env["MOLT_SESSION_ID"] = "pytest-root-normalization"
    env["CARGO_TARGET_DIR"] = str(alt_root / "target")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = env["CARGO_TARGET_DIR"]
    env["MOLT_CACHE"] = str(cache_dir)
    env["MOLT_DIFF_ROOT"] = str(alt_root / "tmp" / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(alt_root / "tmp")
    env["UV_CACHE_DIR"] = str(alt_root / ".uv-cache")
    env["TMPDIR"] = str(alt_root / "tmp")
    env["MOLT_BACKEND_DAEMON"] = "0"
    env["MOLT_EXT_ROOT"] = str(ROOT)

    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src_path),
            "--target",
            "native",
            "--build-profile",
            "dev",
            "--output",
            str(out_path),
        ],
        cwd=alt_root,
        env=env,
        capture_output=True,
        text=True,
        timeout=600,
    )

    assert build.returncode == 0, build.stdout + build.stderr

    run = subprocess.run(
        [str(out_path)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"
