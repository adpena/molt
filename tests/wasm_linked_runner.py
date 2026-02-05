from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest


def _select_out_dir(default: Path, root: Path) -> Path:
    external_root = Path("/Volumes/APDataStore/Molt")
    if external_root.exists():
        try:
            if default.is_relative_to(external_root):
                return default
        except AttributeError:
            # Python <3.9 fallback; not expected but keep safe.
            if str(default).startswith(str(external_root)):
                return default
        base = external_root / "tmp"
        try:
            base.mkdir(parents=True, exist_ok=True)
            return Path(tempfile.mkdtemp(prefix="molt_wasm_", dir=base))
        except OSError:
            pass
    try:
        if default.is_relative_to(root):
            return default
    except AttributeError:
        if str(default).startswith(str(root)):
            return default
    base = root / "build" / "wasm"
    base.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix="molt_wasm_", dir=base))
    return default


def require_wasm_toolchain() -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")
    if shutil.which("wasm-ld") is None:
        pytest.skip("wasm-ld is required for linked wasm parity test")


def build_wasm_linked(
    root: Path,
    src: Path,
    out_dir: Path,
    *,
    extra_args: list[str] | None = None,
) -> Path:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    external_root = Path("/Volumes/APDataStore/Molt")
    if external_root.exists():
        env.setdefault("CARGO_TARGET_DIR", str(external_root / "target"))
    out_dir = _select_out_dir(out_dir, root)
    args = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(src),
        "--target",
        "wasm",
        "--require-linked",
        "--out-dir",
        str(out_dir),
    ]
    if extra_args:
        args.extend(extra_args)
    build = subprocess.run(
        args,
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr
    output_wasm = out_dir / "output_linked.wasm"
    assert output_wasm.exists(), "linked wasm output missing"
    return output_wasm


def run_wasm_linked(
    root: Path, wasm_path: Path, *, env_overrides: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    if env_overrides:
        env.update(env_overrides)
    runner = root / "run_wasm.js"
    return subprocess.run(
        ["node", str(runner), str(wasm_path)],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
