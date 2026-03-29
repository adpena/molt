from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _build_split(
    source_file: Path,
    output_dir: Path,
    *,
    extra_env: dict[str, str] | None = None,
    cargo_target_dir: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    repo_src = str(ROOT / "src")
    current_pythonpath = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = (
        repo_src + os.pathsep + current_pythonpath if current_pythonpath else repo_src
    )
    env["MOLT_BACKEND_DAEMON"] = "0"
    if cargo_target_dir is not None:
        env["CARGO_TARGET_DIR"] = str(cargo_target_dir)
        env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(cargo_target_dir)
    if extra_env:
        env.update(extra_env)
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(source_file),
        "--target",
        "wasm",
        "--split-runtime",
        "--no-cache",
        "--out-dir",
        str(output_dir),
    ]
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        env=env,
        cwd=str(ROOT),
        timeout=300,
    )


def _run_split_direct(output_dir: Path) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["MOLT_WASM_DIRECT_LINK"] = "1"
    env["MOLT_WASM_PREFER_LINKED"] = "0"
    env["MOLT_RUNTIME_WASM"] = str(output_dir / "molt_runtime.wasm")
    return subprocess.run(
        ["node", "wasm/run_wasm.js", str(output_dir / "app.wasm")],
        capture_output=True,
        text=True,
        env=env,
        cwd=str(ROOT),
        timeout=300,
    )


@pytest.mark.slow
def test_split_runtime_imported_module_function_attr_survives_publication(
    tmp_path: Path,
) -> None:
    module_src = tmp_path / "probe_mod.py"
    module_src.write_text(
        "def foo():\n"
        "    return 7\n"
    )
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "import probe_mod\n"
        "print(probe_mod.foo)\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    target_dir = ROOT / "target" / "pytest" / tmp_path.name
    target_dir.mkdir(parents=True, exist_ok=True)

    build = _build_split(
        main_src,
        out_dir,
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
        cargo_target_dir=target_dir,
    )
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "<function>\n"


@pytest.mark.slow
def test_split_runtime_import_os_exposes_open_flags(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "import os\n"
        "print(os.O_RDONLY)\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    target_dir = ROOT / "target" / "pytest" / tmp_path.name
    target_dir.mkdir(parents=True, exist_ok=True)

    build = _build_split(main_src, out_dir, cargo_target_dir=target_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "0\n"
