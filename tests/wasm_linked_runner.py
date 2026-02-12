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
_MIN_NODE_MAJOR = 18
_NODE_BIN_CACHE: str | None = None


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


def _parse_node_major(version_text: str) -> int | None:
    text = version_text.strip()
    if text.startswith("v"):
        text = text[1:]
    head = text.split(".", 1)[0]
    try:
        return int(head)
    except ValueError:
        return None


def _node_major_for_binary(path: str) -> int | None:
    try:
        res = subprocess.run(
            [path, "-p", "process.versions.node"],
            capture_output=True,
            text=True,
        )
    except OSError:
        return None
    if res.returncode != 0:
        return None
    return _parse_node_major(res.stdout)


def _select_node_binary() -> str | None:
    global _NODE_BIN_CACHE
    if _NODE_BIN_CACHE is not None:
        return _NODE_BIN_CACHE

    requested = os.environ.get("MOLT_NODE_BIN", "").strip()
    if requested:
        major = _node_major_for_binary(requested)
        if major is None or major < _MIN_NODE_MAJOR:
            return None
        _NODE_BIN_CACHE = requested
        return requested

    candidates: list[str] = []
    seen: set[str] = set()
    for candidate in (
        shutil.which("node"),
        "/opt/homebrew/bin/node",
        "/usr/local/bin/node",
    ):
        if not candidate or candidate in seen:
            continue
        seen.add(candidate)
        candidates.append(candidate)

    best_path: str | None = None
    best_major = -1
    for candidate in candidates:
        major = _node_major_for_binary(candidate)
        if major is None:
            continue
        if major > best_major:
            best_major = major
            best_path = candidate
    if best_path is None or best_major < _MIN_NODE_MAJOR:
        return None
    _NODE_BIN_CACHE = best_path
    return best_path


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
    node_bin = _select_node_binary()
    if node_bin is None:
        pytest.skip(
            "Node >= 18 is required for wasm parity test (or set MOLT_NODE_BIN)."
        )
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
    env.setdefault("NODE_NO_WARNINGS", "1")
    if env_overrides:
        env.update(env_overrides)
    node_bin = _select_node_binary()
    if node_bin is None:
        raise AssertionError("Node >= 18 is required for wasm execution.")
    runner = root / "run_wasm.js"
    node_args = [
        node_bin,
        "--no-warnings",
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
