from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

_WASM_TEST_LANE = (
    os.environ.get("MOLT_WASM_TEST_LANE", "").strip() or f"lane_{os.getpid()}"
)


def _select_out_dir(default: Path, root: Path) -> Path:
    external_root = Path("/Volumes/APDataStore/Molt")
    use_external = os.environ.get("MOLT_WASM_TEST_USE_EXTERNAL", "").strip().lower()
    allow_external = use_external not in {"0", "false", "no", "off"}
    if allow_external and external_root.exists():
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


def _read_timeout_seconds(env_name: str, default: float) -> float:
    raw = os.environ.get(env_name, "").strip()
    if not raw:
        return default
    try:
        parsed = float(raw)
    except ValueError:
        return default
    if parsed <= 0:
        return default
    return parsed


def _wasm_test_target_dir(root: Path, out_dir: Path, external_root: Path) -> Path:
    override = os.environ.get("MOLT_WASM_TEST_CARGO_TARGET_DIR", "").strip()
    if override:
        target = Path(override).expanduser()
        target.mkdir(parents=True, exist_ok=True)
        return target
    # Keep wasm parity tests isolated from shared benchmark/build lanes that can
    # hold long-lived build locks under CARGO_TARGET_DIR.
    use_external = os.environ.get("MOLT_WASM_TEST_USE_EXTERNAL", "").strip().lower()
    allow_external = use_external not in {"0", "false", "no", "off"}
    if allow_external and external_root.exists():
        target = external_root / "target"
    else:
        target = root / "target" / "pytest_wasm" / _WASM_TEST_LANE
    target.mkdir(parents=True, exist_ok=True)
    return target


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
    use_external = os.environ.get("MOLT_WASM_TEST_USE_EXTERNAL", "").strip().lower()
    allow_external = use_external not in {"0", "false", "no", "off"}
    out_dir = _select_out_dir(out_dir, root)
    env["CARGO_TARGET_DIR"] = str(_wasm_test_target_dir(root, out_dir, external_root))
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_MIDEND_MAX_ROUNDS", "2")
    env.setdefault("MOLT_CSE_MAX_ITERS", "6")
    env.setdefault("MOLT_MIDEND_IDEMPOTENCE_CHECK", "0")
    env.setdefault("MOLT_MIDEND_FAIL_OPEN", "1")
    env.setdefault("MOLT_MIDEND_DISABLE", "1")
    if allow_external and external_root.exists():
        tmp_root = external_root / "tmp"
        tmp_root.mkdir(parents=True, exist_ok=True)
        env.setdefault("TMPDIR", str(tmp_root))
        env.setdefault("MOLT_HOME", str(external_root))
        env.setdefault("MOLT_CACHE", str(external_root / "molt_cache"))
        env.setdefault("MOLT_WASM_RUNTIME_DIR", str(external_root / "wasm"))
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
    build_timeout = _read_timeout_seconds("MOLT_WASM_TEST_BUILD_TIMEOUT_SEC", 900.0)
    try:
        build = subprocess.run(
            args,
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            timeout=build_timeout,
        )
    except subprocess.TimeoutExpired as exc:
        stderr = exc.stderr or ""
        stdout = exc.stdout or ""
        raise AssertionError(
            "WASM build timed out "
            f"after {build_timeout:.1f}s for {src}; "
            f"CARGO_TARGET_DIR={env['CARGO_TARGET_DIR']}\n"
            f"stdout:\n{stdout}\n\nstderr:\n{stderr}"
        ) from exc
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
    node_args = [
        "node",
        "--no-wasm-tier-up",
        "--no-wasm-dynamic-tiering",
        "--wasm-num-compilation-tasks=1",
        str(runner),
        str(wasm_path),
    ]
    run_timeout = _read_timeout_seconds("MOLT_WASM_TEST_RUN_TIMEOUT_SEC", 120.0)
    try:
        return subprocess.run(
            node_args,
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            timeout=run_timeout,
        )
    except subprocess.TimeoutExpired as exc:
        stderr = exc.stderr or ""
        stdout = exc.stdout or ""
        raise AssertionError(
            "WASM execution timed out "
            f"after {run_timeout:.1f}s for {wasm_path}\n"
            f"stdout:\n{stdout}\n\nstderr:\n{stderr}"
        ) from exc
