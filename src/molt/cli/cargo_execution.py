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

# Peak resident memory a single rustc codegen job for the heavy molt-runtime
# (and source-recompiled numpy/scipy) wasm build can reach. Cargo's default
# `-j<num_cpus>` runs that many rustc processes in parallel, so on a small box
# (8GB) an unbounded job count thrashes swap and stalls the build for ~45min.
# Bounding jobs to available memory keeps a wasm build inside an 8GB ceiling.
_BYTES_PER_CARGO_JOB = 2 * 1024 * 1024 * 1024
# Reserve headroom for the OS, the linker (wasm-ld/lld peaks separately from the
# parallel rustc jobs), sccache, and the driving Python before dividing the rest
# among parallel jobs, so the ceiling is a real fit rather than a swap-inducing
# exact division.
_CARGO_JOB_MEMORY_HEADROOM = 2 * 1024 * 1024 * 1024


def _total_system_memory_bytes() -> int | None:
    """Best-effort total physical memory in bytes, or ``None`` if unknown.

    Uses only stdlib probes so the CLI does not depend on ``psutil`` or the
    ``tools/`` memory-guard package (a layering boundary).
    """
    if os.name == "nt":
        import ctypes

        class _MemoryStatusEx(ctypes.Structure):
            _fields_ = [
                ("dwLength", ctypes.c_ulong),
                ("dwMemoryLoad", ctypes.c_ulong),
                ("ullTotalPhys", ctypes.c_ulonglong),
                ("ullAvailPhys", ctypes.c_ulonglong),
                ("ullTotalPageFile", ctypes.c_ulonglong),
                ("ullAvailPageFile", ctypes.c_ulonglong),
                ("ullTotalVirtual", ctypes.c_ulonglong),
                ("ullAvailVirtual", ctypes.c_ulonglong),
                ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
            ]

        status = _MemoryStatusEx()
        status.dwLength = ctypes.sizeof(_MemoryStatusEx)
        try:
            if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
                return int(status.ullTotalPhys)
        except (OSError, AttributeError, ValueError):
            return None
        return None
    try:
        page_size = os.sysconf("SC_PAGE_SIZE")
        phys_pages = os.sysconf("SC_PHYS_PAGES")
    except (ValueError, OSError, AttributeError):
        return None
    if page_size <= 0 or phys_pages <= 0:
        return None
    return int(page_size) * int(phys_pages)


def _memory_bounded_cargo_jobs() -> int | None:
    """Cargo ``--jobs`` ceiling that fits total memory, or ``None`` if unknown.

    Caps parallel rustc jobs to roughly one per ``_BYTES_PER_CARGO_JOB`` of
    total RAM, never exceeding the CPU count. Returns ``None`` when memory can't
    be probed so callers leave cargo's default job count untouched.
    """
    total = _total_system_memory_bytes()
    if total is None:
        return None
    cpu_count = os.cpu_count() or 1
    usable = max(0, total - _CARGO_JOB_MEMORY_HEADROOM)
    mem_jobs = max(1, usable // _BYTES_PER_CARGO_JOB)
    return max(1, min(cpu_count, mem_jobs))


def _apply_memory_bounded_cargo_jobs(env: dict[str, str]) -> None:
    """Default ``CARGO_BUILD_JOBS`` to a memory-fit ceiling when unset.

    An explicit operator/DX ``CARGO_BUILD_JOBS`` always wins; this only supplies
    a safe default for a plain ``molt build`` that would otherwise inherit
    cargo's memory-oblivious ``-j<num_cpus>`` and thrash on a small box.
    """
    if env.get("CARGO_BUILD_JOBS", "").strip():
        return
    jobs = _memory_bounded_cargo_jobs()
    if jobs is not None:
        env["CARGO_BUILD_JOBS"] = str(jobs)


def _maybe_enable_native_cpu(env: dict[str, str]) -> None:
    if env.get("MOLT_NATIVE_CPU", "").strip().lower() in ("1", "true", "yes"):
        existing = env.get("CARGO_BUILD_RUSTFLAGS", env.get("RUSTFLAGS", ""))
        if "target-cpu" not in existing:
            flags = f"{existing} -C target-cpu=native".strip()
            env["CARGO_BUILD_RUSTFLAGS"] = flags


_SCCACHE_DIAG_EMITTED = False


def _sccache_diag(msg: str) -> None:
    """Emit a one-time-per-process LOUD diagnostic about sccache enablement.

    A configured-but-ineffective compiler cache is a silent-degradation trap
    (measured 2026-07-07: 0 requests / 0 hits + mid-compile crashes on Windows
    turning cacheable builds into rc=124 timeouts), so every enable/skip decision
    is announced rather than hidden.
    """
    global _SCCACHE_DIAG_EMITTED
    if _SCCACHE_DIAG_EMITTED:
        return
    _SCCACHE_DIAG_EMITTED = True
    print(f"[molt sccache] {msg}", file=sys.stderr, flush=True)


def _sccache_server_responsive(sccache: str) -> bool:
    """Fast healthcheck: does the sccache server answer at all? Catches a dead
    server. A server that answers --show-stats but crashes mid-compile is caught
    by the retry-degrade in `_run_cargo_with_sccache_retry`."""
    try:
        result = subprocess.run(
            [sccache, "--show-stats"], capture_output=True, text=True, timeout=15
        )
        return result.returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def _maybe_enable_sccache(env: dict[str, str]) -> None:
    if env.get("RUSTC_WRAPPER"):
        return
    mode = env.get("MOLT_USE_SCCACHE", "auto").strip().lower()
    if mode in {"0", "false", "no", "off"}:
        return
    forced = mode in {"1", "true", "yes", "on"}
    sccache = shutil.which("sccache")
    if sccache is None:
        if forced:
            _sccache_diag("MOLT_USE_SCCACHE set but sccache is not on PATH; using direct rustc.")
        return
    # sccache delivers 0 cache hits on this Windows host and crashes builds
    # mid-compile (os error 10054), converting cacheable compiler builds into
    # multi-minute timeouts + manual reruns. Default it OFF on Windows (no value
    # lost) unless explicitly forced; power users can set MOLT_USE_SCCACHE=1.
    if os.name == "nt" and not forced:
        _sccache_diag(
            "disabled by default on Windows (0 cache hits + mid-compile crashes here); "
            "set MOLT_USE_SCCACHE=1 to force. Using direct rustc."
        )
        return
    if not _sccache_server_responsive(sccache):
        _sccache_diag("server healthcheck failed; using direct rustc (set MOLT_USE_SCCACHE=0 to silence).")
        return
    root = _find_molt_root(Path.cwd()) or Path.cwd()
    ext_root = Path(env.get("MOLT_EXT_ROOT", root)).expanduser()
    if not ext_root.is_absolute():
        ext_root = root / ext_root
    env.setdefault("SCCACHE_DIR", str((ext_root / ".sccache").resolve()))
    env.setdefault("SCCACHE_CACHE_SIZE", DEFAULT_SCCACHE_CACHE_SIZE)
    env["RUSTC_WRAPPER"] = sccache
    # sccache SKIPS incremental compilation units, so with sccache enabled we must
    # turn incremental off (else the wrapper caches nothing). When sccache is OFF
    # (the Windows default now), incremental stays ON — see _cargo_build_env.
    env["CARGO_INCREMENTAL"] = "0"
    _sccache_diag(f"enabled (RUSTC_WRAPPER={sccache}); post-build stats attest effectiveness.")


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
    # Incremental compilation is the primary WARM-REBUILD accelerator: edit one
    # file -> recompile only that crate's changed codegen units, not the whole
    # runtime cold. Default it ON. It is mutually exclusive with sccache (which
    # skips incremental units), so the sccache-enable paths force it back to "0";
    # with sccache OFF (the Windows default) incremental is the ONLY compiler
    # cache we have and must be on, else every rebuild pays the full cold compile.
    # An explicit operator-provided CARGO_INCREMENTAL always wins (setdefault).
    if Path(env.get("RUSTC_WRAPPER", "") or "").name == "sccache":
        env.setdefault("CARGO_INCREMENTAL", "0")
    else:
        env.setdefault("CARGO_INCREMENTAL", "1")
    if sys.executable:
        env.setdefault("MOLT_BUILD_PYTHON", sys.executable)
    _apply_memory_bounded_cargo_jobs(env)
    return env


def _attest_sccache_stats(sccache: str, label: str) -> None:
    """Log post-build sccache stats so a configured-but-0-hit cache is VISIBLE
    (the 'sccache was on but doing nothing' class) instead of silently wasting
    the wrapper overhead. Cumulative counts: requests==0 after a build means the
    cache did nothing."""
    try:
        result = subprocess.run(
            [sccache, "--show-stats"], capture_output=True, text=True, timeout=15
        )
    except (OSError, subprocess.SubprocessError):
        return
    if result.returncode != 0:
        return
    requests = hits = "?"
    for line in result.stdout.splitlines():
        low = line.lower()
        if "compile requests" in low and "executed" not in low:
            requests = line.split()[-1]
        elif "cache hits" in low and "rate" not in low:
            hits = line.split()[-1]
    print(
        f"{label}: sccache attest — compile_requests={requests} cache_hits={hits} "
        f"(requests=0 => cache ineffective this session)",
        file=sys.stderr,
        flush=True,
    )


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
    active_wrapper = env.get("RUSTC_WRAPPER", "")
    if not json_output and active_wrapper and Path(active_wrapper).name == "sccache":
        _attest_sccache_stats(active_wrapper, label)
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
