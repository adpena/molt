from __future__ import annotations

import contextlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Iterator

from molt.dx import (
    DEFAULT_SCCACHE_CACHE_SIZE,
    development_artifact_env,
    development_artifacts_requested,
)
from molt.cli.build_locks import _release_file_lock, _try_acquire_file_lock
from molt.cli.command_runtime import _run_completed_command
from molt.cli.project_roots import _find_molt_root


_MAX_CONCURRENT_BUILDS = 2


def _maybe_enable_native_cpu(env: dict[str, str]) -> None:
    if env.get("MOLT_NATIVE_CPU", "").strip().lower() in ("1", "true", "yes"):
        existing = env.get("CARGO_BUILD_RUSTFLAGS", env.get("RUSTFLAGS", ""))
        if "target-cpu" not in existing:
            flags = f"{existing} -C target-cpu=native".strip()
            env["CARGO_BUILD_RUSTFLAGS"] = flags


def _maybe_enable_sccache(env: dict[str, str]) -> None:
    if env.get("RUSTC_WRAPPER"):
        return
    mode = env.get("MOLT_USE_SCCACHE", "auto").strip().lower()
    if mode in {"0", "false", "no", "off"}:
        return
    sccache = shutil.which("sccache")
    if sccache is None:
        return
    root = _find_molt_root(Path.cwd()) or Path.cwd()
    ext_root = Path(env.get("MOLT_EXT_ROOT", root)).expanduser()
    if not ext_root.is_absolute():
        ext_root = root / ext_root
    env.setdefault("SCCACHE_DIR", str((ext_root / ".sccache").resolve()))
    env.setdefault("SCCACHE_CACHE_SIZE", DEFAULT_SCCACHE_CACHE_SIZE)
    env["RUSTC_WRAPPER"] = sccache


def _cargo_build_env() -> dict[str, str]:
    env = os.environ.copy()
    if development_artifacts_requested(env):
        root = _find_molt_root(Path.cwd()) or Path.cwd()
        env = development_artifact_env(
            root,
            env,
            session_prefix="cargo-build",
            session_id=env.get("MOLT_SESSION_ID") or f"cargo-build-{os.getpid()}",
            create_dirs=True,
        )
    env.setdefault("CARGO_INCREMENTAL", "0")
    if sys.executable:
        env.setdefault("MOLT_BUILD_PYTHON", sys.executable)
    return env


def _is_sccache_wrapper_failure(result: subprocess.CompletedProcess[str]) -> bool:
    stderr = result.stderr or ""
    stdout = result.stdout or ""
    combined = f"{stderr}\n{stdout}"
    return "sccache: error:" in combined or (
        "process didn't exit successfully" in combined and "sccache" in combined
    )


def _run_cargo_with_sccache_retry(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: float | None,
    json_output: bool,
    label: str,
) -> subprocess.CompletedProcess[str]:
    build = _run_completed_command(
        cmd,
        cwd=cwd,
        env=env,
        capture_output=True,
        memory_guard_prefix="MOLT_BUILD",
        timeout=timeout,
    )
    wrapper = env.get("RUSTC_WRAPPER", "")
    if (
        build.returncode != 0
        and wrapper
        and Path(wrapper).name == "sccache"
        and _is_sccache_wrapper_failure(build)
    ):
        retry_env = env.copy()
        retry_env.pop("RUSTC_WRAPPER", None)
        if not json_output:
            print(
                f"{label}: sccache wrapper failure detected; retrying without sccache.",
                file=sys.stderr,
            )
        build = _run_completed_command(
            cmd,
            cwd=cwd,
            env=retry_env,
            capture_output=True,
            memory_guard_prefix="MOLT_BUILD",
            timeout=timeout,
        )
    return build


def _build_slot_dir() -> Path:
    tmp_root = (
        os.environ.get("MOLT_DIFF_TMPDIR", "").strip()
        or os.environ.get("TMPDIR", "").strip()
        or os.environ.get("TMP", "").strip()
        or os.environ.get("TEMP", "").strip()
    )
    if tmp_root:
        return Path(tmp_root).expanduser() / "molt-build-slots"
    ext_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if ext_root:
        return Path(ext_root).expanduser() / "tmp" / "molt-build-slots"
    root = _find_molt_root(Path.cwd())
    if root is None:
        root = Path.cwd()
    return root / "tmp" / "molt-build-slots"


@contextlib.contextmanager
def _build_slot() -> Iterator[int]:
    build_slot_dir = _build_slot_dir()
    max_slots_raw = os.environ.get("MOLT_MAX_CONCURRENT_BUILDS", "").strip()
    try:
        max_slots = int(max_slots_raw) if max_slots_raw else _MAX_CONCURRENT_BUILDS
    except ValueError:
        max_slots = _MAX_CONCURRENT_BUILDS
    max_slots = max(1, max_slots)

    while True:
        for slot_idx in range(max_slots):
            slot_path = build_slot_dir / f"slot-{slot_idx}.lock"
            handle = _try_acquire_file_lock(slot_path)
            if handle is None:
                continue
            try:
                yield slot_idx
            finally:
                _release_file_lock(handle)
            return
        time.sleep(0.05)
