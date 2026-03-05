from __future__ import annotations

import contextlib
import os
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, TextIO

try:  # pragma: no cover - platform-specific import.
    import fcntl
except Exception:  # pragma: no cover - non-posix fallback.
    fcntl = None  # type: ignore[assignment]


DEFAULT_MAX_COMPILE_SLOTS = 2
DEFAULT_WAIT_SECONDS = 180.0
DEFAULT_POLL_SECONDS = 0.5
DEFAULT_MAX_LOAD_PER_CPU = 1.25
DEFAULT_MAX_ACTIVE_PROCS_FACTOR = 3

_COMPILE_MATCH_TOKENS = (
    "molt.cli build",
    "cargo build",
    "/rustc",
)


@dataclass
class CompileSlotLease:
    slot_index: int | None
    lock_path: Path | None
    waited_seconds: float
    active_builds: int | None
    load_1m: float | None
    _lock_handle: TextIO | None = None
    _released: bool = False

    def release(self) -> None:
        if self._released:
            return
        self._released = True
        handle = self._lock_handle
        self._lock_handle = None
        if handle is None:
            return
        if fcntl is not None:
            with contextlib.suppress(OSError):
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        with contextlib.suppress(OSError):
            handle.close()


def _parse_bool(value: str | None, default: bool) -> bool:
    if value is None:
        return default
    raw = value.strip().lower()
    if raw in {"1", "true", "yes", "on"}:
        return True
    if raw in {"0", "false", "no", "off"}:
        return False
    return default


def _parse_int(value: str | None, default: int, *, min_value: int = 1) -> int:
    if value is None:
        return default
    try:
        parsed = int(value.strip())
    except (TypeError, ValueError):
        return default
    return max(min_value, parsed)


def _parse_float(value: str | None, default: float, *, min_value: float = 0.0) -> float:
    if value is None:
        return default
    try:
        parsed = float(value.strip())
    except (TypeError, ValueError):
        return default
    return max(min_value, parsed)


def _emit(message: str, *, log: TextIO | None) -> None:
    if log is not None:
        print(message, file=log, flush=True)
        return
    print(message, file=sys.stderr, flush=True)


def _guard_root(env: Mapping[str, str]) -> Path:
    explicit = env.get("MOLT_COMPILE_GUARD_DIR")
    if explicit:
        return Path(explicit).expanduser()
    ext_root = env.get("MOLT_EXT_ROOT")
    if ext_root:
        return (
            Path(ext_root).expanduser()
            / "cargo-target"
            / ".molt_state"
            / "compile_guard"
        )
    target_root = env.get("CARGO_TARGET_DIR")
    if target_root:
        return Path(target_root).expanduser() / ".molt_state" / "compile_guard"
    return Path("/tmp/molt_compile_guard")


def _count_active_compile_processes() -> int | None:
    if os.name != "posix":
        return None
    try:
        res = subprocess.run(
            ["ps", "-ax", "-o", "pid=,command="],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if res.returncode != 0:
        return None
    current_pid = os.getpid()
    count = 0
    for line in res.stdout.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        parts = stripped.split(maxsplit=1)
        if not parts:
            continue
        try:
            pid = int(parts[0])
        except ValueError:
            continue
        if pid == current_pid:
            continue
        cmd = parts[1] if len(parts) > 1 else ""
        if any(token in cmd for token in _COMPILE_MATCH_TOKENS):
            count += 1
    return count


def _load_1m() -> float | None:
    try:
        return float(os.getloadavg()[0])
    except OSError:
        return None


def _max_slots_from_env(env: Mapping[str, str]) -> int:
    return _parse_int(
        env.get("MOLT_COMPILE_MAX_CONCURRENT_BUILDS")
        or env.get("MOLT_COMPILE_GUARD_MAX_SLOTS")
        or env.get("MOLT_MAX_CONCURRENT_AGENTS"),
        DEFAULT_MAX_COMPILE_SLOTS,
        min_value=1,
    )


def _max_load_from_env(env: Mapping[str, str], *, max_slots: int) -> float:
    cpu_count = max(1, os.cpu_count() or 1)
    default_load = max(
        float(max_slots),
        float(cpu_count) * DEFAULT_MAX_LOAD_PER_CPU,
    )
    return _parse_float(
        env.get("MOLT_COMPILE_GUARD_MAX_LOAD"),
        default_load,
        min_value=0.0,
    )


def _max_active_procs_from_env(env: Mapping[str, str], *, max_slots: int) -> int:
    default_limit = max_slots * DEFAULT_MAX_ACTIVE_PROCS_FACTOR
    return _parse_int(
        env.get("MOLT_COMPILE_GUARD_MAX_ACTIVE_PROCS"),
        default_limit,
        min_value=1,
    )


def _try_acquire_slot(
    lock_root: Path, *, max_slots: int
) -> tuple[int, Path, TextIO] | None:
    if os.name != "posix" or fcntl is None:
        return None
    lock_root.mkdir(parents=True, exist_ok=True)
    for slot_index in range(max_slots):
        lock_path = lock_root / f"slot_{slot_index}.lock"
        handle = open(lock_path, "a+", encoding="utf-8")
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            handle.close()
            continue
        except OSError:
            handle.close()
            continue
        handle.seek(0)
        handle.truncate(0)
        handle.write(f"pid={os.getpid()} acquired_at={time.time():.6f}\n")
        handle.flush()
        return slot_index, lock_path, handle
    return None


def acquire_compile_slot(
    *,
    env: Mapping[str, str] | None = None,
    label: str,
    log: TextIO | None = None,
) -> CompileSlotLease:
    env_view = os.environ if env is None else env
    enabled = _parse_bool(env_view.get("MOLT_COMPILE_GUARD_ENABLED"), True)
    if not enabled:
        return CompileSlotLease(
            slot_index=None,
            lock_path=None,
            waited_seconds=0.0,
            active_builds=None,
            load_1m=None,
        )

    max_slots = _max_slots_from_env(env_view)
    max_load = _max_load_from_env(env_view, max_slots=max_slots)
    max_active_procs = _max_active_procs_from_env(env_view, max_slots=max_slots)
    wait_seconds = _parse_float(
        env_view.get("MOLT_COMPILE_GUARD_WAIT_SEC"),
        DEFAULT_WAIT_SECONDS,
        min_value=0.0,
    )
    poll_seconds = _parse_float(
        env_view.get("MOLT_COMPILE_GUARD_POLL_SEC"),
        DEFAULT_POLL_SECONDS,
        min_value=0.05,
    )
    lock_root = _guard_root(env_view)

    started = time.monotonic()
    deadline = started + wait_seconds
    last_reason = "unknown"
    last_active: int | None = None
    last_load: float | None = None
    wait_logged = False

    while True:
        now = time.monotonic()
        active_builds = _count_active_compile_processes()
        load_1m = _load_1m()
        last_active = active_builds
        last_load = load_1m

        reasons: list[str] = []
        if active_builds is not None and active_builds >= max_active_procs:
            reasons.append(f"active_builds={active_builds} >= limit={max_active_procs}")
        if load_1m is not None and max_load > 0.0 and load_1m >= max_load:
            reasons.append(f"load1={load_1m:.2f} >= limit={max_load:.2f}")

        if not reasons:
            acquired = _try_acquire_slot(lock_root, max_slots=max_slots)
            if acquired is not None:
                slot_index, lock_path, handle = acquired
                return CompileSlotLease(
                    slot_index=slot_index,
                    lock_path=lock_path,
                    waited_seconds=max(0.0, time.monotonic() - started),
                    active_builds=active_builds,
                    load_1m=load_1m,
                    _lock_handle=handle,
                )
            reasons.append(f"all compile slots busy (max_slots={max_slots})")

        last_reason = ", ".join(reasons)
        if now >= deadline:
            raise RuntimeError(
                "Timed out waiting for compile capacity "
                f"(label={label}, waited={wait_seconds:.1f}s, reason={last_reason}, "
                f"active_builds={last_active}, load1={last_load})"
            )
        if not wait_logged:
            _emit(
                (
                    "[compile-governor] waiting for slot "
                    f"(label={label}, wait_sec={wait_seconds:.1f}, reason={last_reason})"
                ),
                log=log,
            )
            wait_logged = True
        time.sleep(poll_seconds)


@contextlib.contextmanager
def compile_slot(
    *,
    env: Mapping[str, str] | None = None,
    label: str,
    log: TextIO | None = None,
):
    lease = acquire_compile_slot(env=env, label=label, log=log)
    try:
        yield lease
    finally:
        lease.release()
