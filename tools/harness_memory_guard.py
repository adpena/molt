#!/usr/bin/env python3
from __future__ import annotations

import contextlib
import datetime as dt
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Collection, Iterator, Mapping, Sequence

try:
    from tools import memory_guard, process_sentinel
except ModuleNotFoundError:  # pragma: no cover - direct script import from tools/
    import memory_guard  # type: ignore
    import process_sentinel  # type: ignore


DEFAULT_POLL_INTERVAL_SEC = 0.10
TRUE_VALUES = {"1", "true", "yes", "on"}
FALSE_VALUES = {"0", "false", "no", "off"}
DEFAULT_COMMAND_PROFILE_MAX_MB = 16.0
TERMINATED_PGID_TTL_SEC = 60.0
DEFAULT_STALE_ORPHAN_SEC = process_sentinel.DEFAULT_STALE_ORPHAN_SEC
DEFAULT_STALE_PYTEST_SEC = process_sentinel.DEFAULT_STALE_PYTEST_SEC
HARD_RSS_LIMIT_GB = memory_guard.DEFAULT_HARD_MAX_RSS_GB - 0.001
HARD_GLOBAL_RSS_LIMIT_GB = memory_guard.DEFAULT_HARD_MAX_GLOBAL_RSS_GB - 0.001
HARD_CHILD_RLIMIT_GB = memory_guard.DEFAULT_HARD_MAX_CHILD_RLIMIT_GB - 0.001
CODEX_INTERACTIVE_MAX_PROCESS_RSS_GB = 18.0
CODEX_INTERACTIVE_MAX_TOTAL_RSS_GB = 24.0
CODEX_INTERACTIVE_MAX_GLOBAL_RSS_GB = 36.0
_REPO_ROOT = Path(__file__).resolve().parents[1]
_SRC_ROOT = _REPO_ROOT / "src"
if _SRC_ROOT.exists() and str(_SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(_SRC_ROOT))

from molt.dx import (  # noqa: E402
    CANONICAL_ROOT_ENV_KEYS as _CANONICAL_ROOT_ENV_KEYS,
    CANONICAL_RUN_ENV_KEYS as _CANONICAL_RUN_ENV_KEYS,
    RunContext,
)

CANONICAL_ROOT_ENV_KEYS = _CANONICAL_ROOT_ENV_KEYS
CANONICAL_RUN_ENV_KEYS = _CANONICAL_RUN_ENV_KEYS

_TERMINATED_PGIDS: dict[int, float] = {}
_TERMINATED_PGIDS_LOCK = threading.Lock()
_AUTO_SENTINEL_SUPPRESSORS = 0
_AUTO_SENTINEL_SUPPRESSORS_LOCK = threading.Lock()


def canonical_interpreter(executable: str) -> str:
    """Resolve an interpreter command to an absolute, existing path.

    `sys.executable` under `uv run` is an absolute `.venv/bin/python3` symlink
    chain; a relative form (e.g. resolved against a relative repo root by a
    caller) breaks under the memory guard's `cwd`-relative spawn. Resolve to an
    absolute path so the guarded subprocess can exec it regardless of cwd.
    Fail closed with a clear error rather than emit a relative path the guard
    will mis-resolve.

    This is the construction-time complement to
    ``memory_guard._resolve_relative_executable`` (the spawn-time bug-class fix):
    callers canonicalize the interpreter they intend to hand the guard so a
    relative form never reaches the spawn boundary in the first place.
    """
    path = Path(executable)
    if not path.is_absolute():
        resolved = shutil.which(executable)
        if resolved is None:
            raise FileNotFoundError(
                f"CPython baseline interpreter not found on PATH: {executable!r}"
            )
        path = Path(resolved)
    abs_path = path.resolve(strict=False)
    if not abs_path.exists():
        raise FileNotFoundError(f"CPython baseline interpreter missing: {abs_path}")
    return str(abs_path)


class GuardedCompletedProcess(subprocess.CompletedProcess[object]):
    def __init__(
        self,
        args: Sequence[str],
        returncode: int,
        stdout: str | bytes | None,
        stderr: str | bytes | None,
        *,
        elapsed_s: float | None,
        violation: memory_guard.RssViolation | None = None,
        timed_out: bool = False,
        limit_at_violation: memory_guard.ResolvedMemoryLimits | None = None,
        orphaned_process_groups: Sequence[int] = (),
        cargo_incremental_quarantine: (
            memory_guard.CargoIncrementalQuarantine | None
        ) = None,
        child_process: memory_guard.GuardedChildProcess | None = None,
        termination_reports: Sequence[memory_guard.GuardTerminationReport] = (),
        guard_signal: int | None = None,
    ) -> None:
        super().__init__(
            args=list(args), returncode=returncode, stdout=stdout, stderr=stderr
        )
        self.elapsed_s = elapsed_s
        self.violation = violation
        self.timed_out = timed_out
        self.limit_at_violation = limit_at_violation
        self.orphaned_process_groups = tuple(orphaned_process_groups)
        self.cargo_incremental_quarantine = cargo_incremental_quarantine
        self.child_process = child_process
        self.termination_reports = tuple(termination_reports)
        self.guard_signal = guard_signal


def _claim_terminated_pgid(pgid: int) -> bool:
    now = time.monotonic()
    with _TERMINATED_PGIDS_LOCK:
        stale = [
            known_pgid
            for known_pgid, ts in _TERMINATED_PGIDS.items()
            if now - ts > TERMINATED_PGID_TTL_SEC
        ]
        for known_pgid in stale:
            _TERMINATED_PGIDS.pop(known_pgid, None)
        if pgid in _TERMINATED_PGIDS:
            return False
        _TERMINATED_PGIDS[pgid] = now
        return True


def _note_auto_sentinel_suppressor_entered() -> None:
    global _AUTO_SENTINEL_SUPPRESSORS
    with _AUTO_SENTINEL_SUPPRESSORS_LOCK:
        _AUTO_SENTINEL_SUPPRESSORS += 1


def _note_auto_sentinel_suppressor_exited() -> None:
    global _AUTO_SENTINEL_SUPPRESSORS
    with _AUTO_SENTINEL_SUPPRESSORS_LOCK:
        _AUTO_SENTINEL_SUPPRESSORS = max(0, _AUTO_SENTINEL_SUPPRESSORS - 1)


def _sentinel_active() -> bool:
    with _AUTO_SENTINEL_SUPPRESSORS_LOCK:
        return _AUTO_SENTINEL_SUPPRESSORS > 0


def repo_sentinel_active_env_key(prefix: str) -> str:
    normalized = _normalize_prefix(prefix) or "MOLT"
    return f"{normalized}_REPO_SENTINEL_ACTIVE"


def _external_repo_sentinel_active(
    prefix: str,
    env: Mapping[str, str] | None,
) -> bool:
    source = _effective_env(env)
    normalized = _normalize_prefix(prefix) or "MOLT"
    return _env_bool(
        source,
        [repo_sentinel_active_env_key(normalized), "MOLT_REPO_SENTINEL_ACTIVE"],
        default=False,
    )


@dataclass(frozen=True, slots=True)
class HarnessMemoryLimits:
    enabled: bool
    max_process_rss_gb: float
    max_total_rss_gb: float
    max_global_rss_gb: float
    poll_interval: float
    child_rlimit_gb: float | None = None
    adaptive_prefix: str = "MOLT"
    dynamic_process_rss: bool = False
    dynamic_total_rss: bool = False
    dynamic_global_rss: bool = False
    dynamic_child_rlimit: bool = False
    interactive_budget: bool = False
    max_process_rss_kb: int = field(init=False, repr=False)
    max_total_rss_kb: int = field(init=False, repr=False)
    max_global_rss_kb: int = field(init=False, repr=False)
    child_rlimit_kb: int | None = field(init=False, repr=False)

    def __post_init__(self) -> None:
        object.__setattr__(self, "enabled", True)
        object.__setattr__(
            self,
            "max_process_rss_kb",
            memory_guard.max_rss_kb_from_gb(self.max_process_rss_gb),
        )
        object.__setattr__(
            self,
            "max_total_rss_kb",
            memory_guard.max_rss_kb_from_gb(self.max_total_rss_gb),
        )
        object.__setattr__(
            self,
            "max_global_rss_kb",
            memory_guard.max_global_rss_kb_from_gb(self.max_global_rss_gb),
        )
        child_rlimit_gb = self.child_rlimit_gb
        if child_rlimit_gb is None:
            child_rlimit_gb = memory_guard.default_child_rlimit_gb(
                max_process_rss_gb=self.max_process_rss_gb,
                max_total_rss_gb=self.max_total_rss_gb,
                max_global_rss_gb=self.max_global_rss_gb,
            )
            object.__setattr__(self, "child_rlimit_gb", child_rlimit_gb)
        object.__setattr__(
            self,
            "child_rlimit_kb",
            None
            if child_rlimit_gb <= 0
            else memory_guard.child_rlimit_kb_from_gb(child_rlimit_gb),
        )

    def current_memory_limits(
        self,
        env: Mapping[str, str] | None = None,
        *,
        accounted_rss_kb: int = 0,
    ) -> memory_guard.ResolvedMemoryLimits:
        source = _effective_env(env)

        def provider(accounted: int) -> memory_guard.AdaptiveMemoryBudget:
            return memory_guard.adaptive_memory_budget(
                self.adaptive_prefix,
                source,
                accounted_rss_kb=accounted,
            )

        return memory_guard.resolve_memory_limits(
            max_process_rss_kb=self.max_process_rss_kb,
            max_total_rss_kb=self.max_total_rss_kb,
            max_global_rss_kb=self.max_global_rss_kb,
            adaptive_budget_provider=provider,
            dynamic_process_rss=self.dynamic_process_rss,
            dynamic_total_rss=self.dynamic_total_rss,
            dynamic_global_rss=self.dynamic_global_rss,
            accounted_rss_kb=accounted_rss_kb,
        )

    def current_child_rlimit_kb(
        self,
        env: Mapping[str, str] | None = None,
        *,
        accounted_rss_kb: int = 0,
    ) -> int | None:
        if self.child_rlimit_gb is not None and self.child_rlimit_gb <= 0:
            return None
        if not self.dynamic_child_rlimit:
            return self.child_rlimit_kb
        current = self.current_memory_limits(
            env,
            accounted_rss_kb=accounted_rss_kb,
        )
        current_total_gb = current.max_total_rss_gb
        if current_total_gb is None:
            current_total_gb = current.max_process_rss_gb
        child_rlimit_gb = memory_guard.default_child_rlimit_gb(
            max_process_rss_gb=current.max_process_rss_gb,
            max_total_rss_gb=current_total_gb,
            max_global_rss_gb=current.max_global_rss_gb,
        )
        return memory_guard.child_rlimit_kb_from_gb(child_rlimit_gb)


def _normalize_prefix(prefix: str) -> str:
    return prefix.strip().upper().rstrip("_")


def _label_from_prefix(prefix: str) -> str:
    normalized = _normalize_prefix(prefix).lower() or "molt"
    return "".join(ch if ch.isalnum() else "_" for ch in normalized)


def _effective_env(env: Mapping[str, str] | None) -> Mapping[str, str]:
    if env is None:
        return os.environ
    merged = dict(os.environ)
    merged.update(env)
    return merged


def canonical_harness_env(
    env: Mapping[str, str] | None = None,
    *,
    repo_root: Path | None = None,
    force_default_keys: Collection[str] = (),
) -> dict[str, str]:
    """Return a subprocess env with repo-local artifact/cache defaults installed."""

    root = (repo_root or _REPO_ROOT).resolve()
    merged = dict(os.environ) if env is None else dict(env)
    return RunContext(root, session_prefix="guard").canonical_env(
        merged,
        create_dirs=False,
        force_default_keys=force_default_keys,
    )


def _artifact_root_from_env(env: Mapping[str, str] | None) -> Path:
    source = _effective_env(env)
    explicit = source.get("MOLT_EXT_ROOT")
    root = Path(explicit).expanduser() if explicit else _REPO_ROOT
    return root / "tmp" / "harness_memory_guard"


def _env_bool(
    env: Mapping[str, str],
    names: Sequence[str],
    *,
    default: bool,
) -> bool:
    for name in names:
        raw = env.get(name)
        if raw is None:
            continue
        lowered = raw.strip().lower()
        if lowered in TRUE_VALUES:
            return True
        if lowered in FALSE_VALUES:
            return False
    return default


def _env_float(
    env: Mapping[str, str],
    names: Sequence[str],
    *,
    default: float,
) -> float:
    value = _env_float_optional(env, names)
    return default if value is None else value


def _env_float_optional(
    env: Mapping[str, str],
    names: Sequence[str],
) -> float | None:
    for name in names:
        raw = env.get(name)
        if raw is None or not raw.strip():
            continue
        try:
            return float(raw)
        except ValueError:
            continue
    return None


def _clamp_hard_limit(value: float, hard_limit_gb: float) -> float:
    return min(value, hard_limit_gb)


def _codex_interactive_shell(source: Mapping[str, str]) -> bool:
    if source.get("CODEX_SHELL", "").strip() == "1":
        return True
    origin = source.get("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "")
    if "Codex" in origin:
        return True
    return False


def _cap_dynamic_interactive_budget(
    *,
    source: Mapping[str, str],
    process_override: float | None,
    total_override: float | None,
    global_override: float | None,
    process_gb: float,
    total_gb: float,
    global_gb: float,
) -> tuple[float, float, float, bool]:
    if not _codex_interactive_shell(source):
        return process_gb, total_gb, global_gb, False
    capped = False
    if process_override is None:
        process_gb = min(process_gb, CODEX_INTERACTIVE_MAX_PROCESS_RSS_GB)
        capped = True
    if total_override is None:
        total_gb = min(total_gb, CODEX_INTERACTIVE_MAX_TOTAL_RSS_GB)
        capped = True
    if global_override is None:
        global_gb = min(global_gb, CODEX_INTERACTIVE_MAX_GLOBAL_RSS_GB)
        capped = True
    return process_gb, total_gb, global_gb, capped


def enabled_from_env(
    prefix: str,
    env: Mapping[str, str] | None = None,
) -> bool:
    del prefix, env
    return True


def limits_from_env(
    prefix: str,
    env: Mapping[str, str] | None = None,
) -> HarnessMemoryLimits:
    source = _effective_env(env)
    normalized = _normalize_prefix(prefix)
    adaptive_budget = memory_guard.adaptive_memory_budget(normalized, source)
    process_override = _env_float_optional(
        source,
        [
            f"{normalized}_MAX_PROCESS_RSS_GB",
            f"{normalized}_MAX_RSS_GB",
            "MOLT_MAX_PROCESS_RSS_GB",
            "MOLT_MAX_RSS_GB",
        ],
    )
    process_gb = (
        adaptive_budget.max_process_rss_gb
        if process_override is None
        else process_override
    )
    total_override = _env_float_optional(
        source,
        [
            f"{normalized}_MAX_TOTAL_RSS_GB",
            f"{normalized}_MAX_TREE_RSS_GB",
            "MOLT_MAX_TOTAL_RSS_GB",
            "MOLT_MAX_TREE_RSS_GB",
        ],
    )
    total_gb = (
        adaptive_budget.max_total_rss_gb if total_override is None else total_override
    )
    global_override = _env_float_optional(
        source,
        [
            f"{normalized}_GLOBAL_RSS_LIMIT_GB",
            f"{normalized}_MAX_GLOBAL_RSS_GB",
            "MOLT_GLOBAL_RSS_LIMIT_GB",
            "MOLT_MAX_GLOBAL_RSS_GB",
        ],
    )
    global_gb = (
        adaptive_budget.max_global_rss_gb
        if global_override is None
        else global_override
    )
    process_gb, total_gb, global_gb, interactive_budget = (
        _cap_dynamic_interactive_budget(
            source=source,
            process_override=process_override,
            total_override=total_override,
            global_override=global_override,
            process_gb=process_gb,
            total_gb=total_gb,
            global_gb=global_gb,
        )
    )
    global_gb = _clamp_hard_limit(
        global_gb,
        min(HARD_GLOBAL_RSS_LIMIT_GB, adaptive_budget.max_global_rss_gb),
    )
    total_gb = _clamp_hard_limit(total_gb, min(HARD_RSS_LIMIT_GB, global_gb))
    process_gb = _clamp_hard_limit(process_gb, min(HARD_RSS_LIMIT_GB, total_gb))
    poll_interval = _env_float(
        source,
        [f"{normalized}_MEMORY_GUARD_POLL_SEC", "MOLT_MEMORY_GUARD_POLL_SEC"],
        default=DEFAULT_POLL_INTERVAL_SEC,
    )
    if poll_interval <= 0:
        poll_interval = DEFAULT_POLL_INTERVAL_SEC
    child_rlimit_override = _env_float_optional(
        source,
        [
            f"{normalized}_CHILD_RLIMIT_GB",
            f"{normalized}_MAX_CHILD_RLIMIT_GB",
            "MOLT_CHILD_RLIMIT_GB",
            "MOLT_MAX_CHILD_RLIMIT_GB",
        ],
    )
    child_rlimit_gb = (
        memory_guard.default_child_rlimit_gb(
            max_process_rss_gb=process_gb,
            max_total_rss_gb=total_gb,
            max_global_rss_gb=global_gb,
        )
        if child_rlimit_override is None
        else child_rlimit_override
    )
    if child_rlimit_gb > 0:
        child_rlimit_cap_gb = (
            memory_guard.default_child_rlimit_gb(
                max_process_rss_gb=process_gb,
                max_total_rss_gb=total_gb,
                max_global_rss_gb=global_gb,
            )
            if child_rlimit_override is None
            else HARD_CHILD_RLIMIT_GB
        )
        child_rlimit_gb = _clamp_hard_limit(child_rlimit_gb, child_rlimit_cap_gb)
    return HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=process_gb,
        max_total_rss_gb=total_gb,
        max_global_rss_gb=global_gb,
        poll_interval=poll_interval,
        child_rlimit_gb=child_rlimit_gb,
        adaptive_prefix=normalized,
        dynamic_process_rss=process_override is None,
        dynamic_total_rss=total_override is None,
        dynamic_global_rss=global_override is None,
        dynamic_child_rlimit=child_rlimit_override is None,
        interactive_budget=interactive_budget,
    )


def limits_summary(limits: HarnessMemoryLimits) -> dict[str, object]:
    return {
        "enabled": limits.enabled,
        "max_process_rss_gb": limits.max_process_rss_gb,
        "max_total_rss_gb": limits.max_total_rss_gb,
        "max_global_rss_gb": limits.max_global_rss_gb,
        "child_rlimit_gb": limits.child_rlimit_gb,
        "poll_interval": limits.poll_interval,
        "dynamic_process_rss": limits.dynamic_process_rss,
        "dynamic_total_rss": limits.dynamic_total_rss,
        "dynamic_global_rss": limits.dynamic_global_rss,
        "dynamic_child_rlimit": limits.dynamic_child_rlimit,
        "interactive_budget": limits.interactive_budget,
    }


def limits_status_line(limits: HarnessMemoryLimits) -> str:
    return (
        "Memory guard: "
        f"enabled={limits.enabled} "
        f"process={limits.max_process_rss_gb:.2f}GB "
        f"tree={limits.max_total_rss_gb:.2f}GB "
        f"global={limits.max_global_rss_gb:.2f}GB "
        f"interactive={limits.interactive_budget} "
        f"dynamic={limits.dynamic_global_rss}"
    )


def timeout_from_env(
    prefix: str,
    env: Mapping[str, str] | None = None,
    *,
    explicit: float | None = None,
    default: float | None = None,
) -> float | None:
    if explicit is not None:
        return explicit
    source = _effective_env(env)
    normalized = _normalize_prefix(prefix)
    for name in (f"{normalized}_TIMEOUT_SEC", "MOLT_TEST_PROCESS_TIMEOUT_SEC"):
        raw = source.get(name)
        if raw is None or not raw.strip():
            continue
        lowered = raw.strip().lower()
        if lowered in FALSE_VALUES:
            return None
        try:
            parsed = float(raw)
        except ValueError:
            continue
        return parsed if parsed > 0 else None
    return default


def command_profile_log_path(
    env: Mapping[str, str] | None = None,
    *,
    repo_root: Path | None = None,
) -> Path:
    """Return the default structured command-profile log path."""

    source = _effective_env(env)
    root = (repo_root or _REPO_ROOT).resolve()
    raw_path = source.get("MOLT_GUARD_PROFILE_LOG", "").strip()
    if raw_path:
        path = Path(raw_path).expanduser()
        return path if path.is_absolute() else root / path
    return root / "logs" / "harness_memory_guard" / "commands.jsonl"


def _max_bytes_from_mb(value: float | None) -> int | None:
    if value is None:
        return None
    if value <= 0:
        return None
    return max(1024, int(value * 1024 * 1024))


def _rotate_jsonl_if_needed(
    path: Path,
    *,
    incoming_bytes: int,
    max_bytes: int | None,
) -> None:
    if max_bytes is None:
        return
    try:
        current_size = path.stat().st_size
    except (FileNotFoundError, OSError):
        return
    if current_size + incoming_bytes <= max_bytes:
        return
    rotated = path.with_name(f"{path.name}.1")
    with contextlib.suppress(OSError):
        rotated.unlink()
    with contextlib.suppress(OSError):
        path.replace(rotated)


def _append_jsonl(
    path: Path,
    payload: dict[str, object],
    *,
    max_bytes: int | None = None,
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    line = json.dumps(payload, sort_keys=True) + "\n"
    _rotate_jsonl_if_needed(
        path,
        incoming_bytes=len(line.encode("utf-8")),
        max_bytes=max_bytes,
    )
    with path.open("a", encoding="utf-8") as handle:
        handle.write(line)


def _utc_timestamp() -> str:
    return (
        dt.datetime.now(dt.timezone.utc)
        .isoformat(timespec="seconds")
        .replace(
            "+00:00",
            "Z",
        )
    )


def _elapsed_text(elapsed_s: float | None) -> str:
    return "unknown" if elapsed_s is None else f"{elapsed_s:.2f}s"


def _limit_text(limit_gb: float | None) -> str:
    return "unknown" if limit_gb is None else f"{limit_gb:.2f}GB"


def _rss_limit_hint(prefix: str) -> str:
    normalized = _normalize_prefix(prefix) or "MOLT"
    if normalized == "MOLT":
        return "MOLT_MAX_PROCESS_RSS_GB/MOLT_MAX_TOTAL_RSS_GB"
    return (
        f"{normalized}_MAX_PROCESS_RSS_GB/{normalized}_MAX_TOTAL_RSS_GB "
        "or the parent MOLT_MAX_* RSS limits"
    )


def _timeout_hint(prefix: str) -> str:
    normalized = _normalize_prefix(prefix) or "MOLT"
    return f"{normalized}_TIMEOUT_SEC or MOLT_TEST_PROCESS_TIMEOUT_SEC"


def _guard_stderr_message(
    violation: memory_guard.RssViolation | None,
    limits: HarnessMemoryLimits,
    effective_limits: memory_guard.ResolvedMemoryLimits | None = None,
    *,
    prefix: str,
    elapsed_s: float | None,
    killed_at: str,
) -> str:
    if violation is None:
        return ""
    limit_gb = (
        (
            effective_limits.max_total_rss_gb
            if effective_limits is not None
            else limits.max_total_rss_gb
        )
        if violation.scope == "process_tree"
        else (
            effective_limits.max_process_rss_gb
            if effective_limits is not None
            else limits.max_process_rss_gb
        )
    )
    cleanup = (
        "classified the command as failed from child exit resource usage"
        if violation.scope == "process_rusage"
        else "terminated the tracked process tree to prevent orphaned Molt subprocesses"
    )
    time_label = "observed_at" if violation.scope == "process_rusage" else "killed_at"
    return (
        "memory_guard: RSS limit exceeded; "
        f"{cleanup}: {time_label}={killed_at} elapsed={_elapsed_text(elapsed_s)} "
        f"pid={violation.pid} rss={violation.rss_gb:.2f}GB "
        f"limit={_limit_text(limit_gb)} scope={violation.scope} "
        f"command={violation.command}\n"
        "memory_guard: next action: inspect child logs and allocations for runaway "
        "work; lower parallelism/input size, or if this workload is expected raise "
        f"{_rss_limit_hint(prefix)} within repo policy.\n"
    )


def _guard_timeout_message(
    *,
    prefix: str,
    timeout: float | None,
    elapsed_s: float | None,
    killed_at: str,
) -> str:
    timeout_text = "unknown" if timeout is None else f"{timeout:.2f}s"
    return (
        "memory_guard: timeout; terminated the tracked process tree to prevent "
        "orphaned Molt subprocesses: "
        f"killed_at={killed_at} elapsed={_elapsed_text(elapsed_s)} "
        f"timeout={timeout_text}\n"
        "memory_guard: next action: inspect child logs for a hang or oversized "
        f"workload; if intentional raise {_timeout_hint(prefix)} for this guard "
        "family.\n"
    )


def _guard_exit_signal_message(
    returncode: int,
    *,
    elapsed_s: float | None,
    observed_at: str,
) -> str:
    payload = memory_guard.exit_signal_payload(returncode)
    if payload is None:
        return ""
    signame = payload["name"] or f"signal {payload['signal']}"
    return (
        "memory_guard: command exited with "
        f"{signame} status ({returncode}); no RSS violation observed: "
        f"observed_at={observed_at} elapsed={_elapsed_text(elapsed_s)}\n"
        "memory_guard: next action: inspect child stderr/logs or host signal "
        "source, including direct-child resource limits such as RLIMIT_AS; if "
        "host memory pressure was involved, rerun with guard samples and lower "
        "parallelism.\n"
    )


def _guard_parent_signal_message(
    guard_signal: int,
    *,
    elapsed_s: float | None,
    observed_at: str,
    primary_reason: str | None = None,
) -> str:
    payload = memory_guard.exit_signal_payload(128 + guard_signal)
    signame = (
        payload["name"]
        if payload is not None and payload["name"] is not None
        else f"signal {guard_signal}"
    )
    if primary_reason is None:
        return (
            "memory_guard: guard parent received "
            f"{signame}; terminated tracked process tree before exiting: "
            f"observed_at={observed_at} elapsed={_elapsed_text(elapsed_s)}\n"
            "memory_guard: next action: inspect the parent host/control-plane "
            "signal source and child logs; the guard parent received the signal "
            "and wrote this custody record before exiting.\n"
        )
    return (
        "memory_guard: guard parent also received "
        f"{signame} while primary incident remained {primary_reason}: "
        f"observed_at={observed_at} elapsed={_elapsed_text(elapsed_s)}\n"
        "memory_guard: next action: inspect the parent host/control-plane "
        "signal source and child logs; preserve the primary incident "
        "classification when triaging this run.\n"
    )


def _guard_orphan_cleanup_message(
    process_groups: Sequence[int],
    *,
    elapsed_s: float | None,
    killed_at: str,
) -> str:
    if not process_groups:
        return ""
    pgids = ",".join(str(pgid) for pgid in process_groups)
    return (
        "memory_guard: orphaned child processes detected after command exit; "
        "terminated tracked process groups to prevent accumulation: "
        f"killed_at={killed_at} elapsed={_elapsed_text(elapsed_s)} "
        f"pgids={pgids} reason=direct child exited while descendants were still "
        "live\n"
        "memory_guard: next action: inspect child process lifecycle and logs; "
        "make helpers shut down explicitly, or run intentional warm daemons inside "
        "a suite-level sentinel that drains at scope exit.\n"
    )


def _rss_record_payload(
    record: memory_guard.RssViolation | None,
) -> dict[str, object] | None:
    if record is None:
        return None
    return {
        "pid": record.pid,
        "rss_kb": record.rss_kb,
        "rss_gb": record.rss_gb,
        "command": record.command,
        "scope": record.scope,
    }


def _guarded_command_status(
    *,
    returncode: int,
    violation: memory_guard.RssViolation | None,
    timed_out: bool,
    orphaned_process_groups: Sequence[int],
    guard_signal: int | None = None,
) -> str:
    if violation is not None:
        return "rss_limit_exceeded"
    if timed_out:
        return "timeout"
    if guard_signal is not None:
        return "guard_interrupted"
    if memory_guard.exit_signal_payload(returncode) is not None:
        return "signal_exit"
    if returncode != 0:
        return "failed"
    if orphaned_process_groups:
        return "pass_with_orphan_cleanup"
    return "pass"


def _github_context_payload(env: Mapping[str, str]) -> dict[str, str] | None:
    keys = (
        "GITHUB_WORKFLOW",
        "GITHUB_JOB",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "GITHUB_SHA",
        "GITHUB_REF",
    )
    payload = {key: env[key] for key in keys if env.get(key)}
    return payload or None


def _command_profile_mode(env: Mapping[str, str]) -> str:
    raw = (
        (env.get("MOLT_GUARD_PROFILE", "") or env.get("MOLT_GUARD_PROFILE_MODE", ""))
        .strip()
        .lower()
    )
    if raw in FALSE_VALUES:
        return "off"
    if env.get("MOLT_GUARD_PROFILE_LOG", "").strip():
        return "all"
    if raw in {"1", "all", "always", "true", "yes", "on"}:
        return "all"
    if raw in {"incident", "incidents", "failure", "failures"}:
        return "incident"
    return "incident"


def _guard_repro_message(
    *,
    command: Sequence[str],
    cwd: str | Path | None,
    env: Mapping[str, str] | None,
    limits: HarnessMemoryLimits,
    timeout: float | None,
    prefix: str,
) -> str:
    payload = memory_guard.repro_context_payload(
        command=command,
        cwd=cwd,
        environ=_effective_env(env),
        max_process_rss_kb=limits.max_process_rss_kb,
        max_total_rss_kb=limits.max_total_rss_kb,
        max_global_rss_kb=limits.max_global_rss_kb,
        child_rlimit_kb=limits.current_child_rlimit_kb(env),
        timeout_s=timeout,
        poll_interval_s=limits.poll_interval,
        summary_json=None,
    )
    payload["prefix"] = _normalize_prefix(prefix) or "MOLT"
    return f"memory_guard: repro context: {memory_guard.repro_context_line(payload)}\n"


def _append_guarded_command_profile(
    *,
    command: Sequence[str],
    prefix: str,
    cwd: str | Path | None,
    env: Mapping[str, str] | None,
    limits: HarnessMemoryLimits,
    returncode: int,
    elapsed_s: float | None,
    timeout_s: float | None,
    violation: memory_guard.RssViolation | None,
    timed_out: bool,
    limit_at_violation: memory_guard.ResolvedMemoryLimits | None,
    orphaned_process_groups: Sequence[int],
    peak: memory_guard.RssViolation | None = None,
    peak_total: memory_guard.RssViolation | None = None,
    cargo_incremental_quarantine: (
        memory_guard.CargoIncrementalQuarantine | None
    ) = None,
    child_process: memory_guard.GuardedChildProcess | None = None,
    termination_reports: Sequence[memory_guard.GuardTerminationReport] = (),
    guard_signal: int | None = None,
) -> tuple[Path, str | None]:
    source = _effective_env(env)
    path = command_profile_log_path(source)
    max_bytes = _max_bytes_from_mb(
        _env_float(
            source,
            ["MOLT_GUARD_PROFILE_MAX_MB"],
            default=DEFAULT_COMMAND_PROFILE_MAX_MB,
        )
    )
    exit_signal = (
        None
        if violation is not None or timed_out or guard_signal is not None
        else memory_guard.exit_signal_payload(returncode)
    )
    guard_signal_payload = (
        None
        if guard_signal is None
        else memory_guard.exit_signal_payload(128 + guard_signal)
    )
    status = _guarded_command_status(
        returncode=returncode,
        violation=violation,
        timed_out=timed_out,
        orphaned_process_groups=orphaned_process_groups,
        guard_signal=guard_signal,
    )
    mode = _command_profile_mode(source)
    if mode == "off" or (mode == "incident" and status == "pass"):
        return path, None
    payload: dict[str, object] = {
        "schema_version": "1.0",
        "event": "guarded_command_profile",
        "recorded_at": _utc_timestamp(),
        "prefix": _normalize_prefix(prefix) or "MOLT",
        "session_id": source.get("MOLT_SESSION_ID", ""),
        "cwd": str(Path(cwd).expanduser() if cwd is not None else Path.cwd()),
        "command": list(command),
        "returncode": returncode,
        "status": status,
        "elapsed_s": None if elapsed_s is None else round(elapsed_s, 6),
        "memory_guard": limits_summary(limits),
        "memory_guard_enabled": limits.enabled,
        "timed_out": timed_out,
        "violation": _rss_record_payload(violation),
        "peak": _rss_record_payload(peak),
        "peak_total": _rss_record_payload(peak_total),
        "orphaned_process_groups": list(orphaned_process_groups),
        "child_process": memory_guard.guarded_child_process_payload(child_process),
        "termination_reports": memory_guard.termination_reports_payload(
            termination_reports
        ),
        "cargo_incremental_quarantine": (
            memory_guard._cargo_incremental_quarantine_payload(
                cargo_incremental_quarantine
            )
        ),
        "limit_at_violation": (
            None
            if limit_at_violation is None
            else memory_guard.memory_limits_payload(limit_at_violation)
        ),
        "exit_signal": exit_signal,
        "guard_signal": guard_signal_payload,
    }
    if status != "pass":
        payload["repro"] = memory_guard.repro_context_payload(
            command=command,
            cwd=cwd,
            environ=source,
            max_process_rss_kb=limits.max_process_rss_kb,
            max_total_rss_kb=limits.max_total_rss_kb,
            max_global_rss_kb=(
                limit_at_violation.max_global_rss_kb
                if limit_at_violation is not None
                else limits.max_global_rss_kb
            ),
            child_rlimit_kb=limits.current_child_rlimit_kb(env),
            timeout_s=timeout_s,
            poll_interval_s=limits.poll_interval,
            summary_json=None,
        )
    github_context = _github_context_payload(source)
    if github_context is not None:
        payload["github"] = github_context
    try:
        _append_jsonl(path, payload, max_bytes=max_bytes)
    except OSError as exc:
        return (
            path,
            f"memory_guard: command profile write failed: path={path} error={exc}\n",
        )
    return path, None


def _stale_orphan_cleanup_enabled(
    prefix: str,
    env: Mapping[str, str] | None,
) -> bool:
    source = _effective_env(env)
    normalized = _normalize_prefix(prefix)
    return _env_bool(
        source,
        [f"{normalized}_STALE_ORPHAN_CLEANUP", "MOLT_STALE_ORPHAN_CLEANUP"],
        default=True,
    )


def _stale_seconds_from_env(
    prefix: str,
    env: Mapping[str, str] | None,
    *,
    suffix: str,
    default: float,
) -> float | None:
    source = _effective_env(env)
    normalized = _normalize_prefix(prefix)
    value = _env_float_optional(
        source,
        [f"{normalized}_{suffix}", f"MOLT_{suffix}"],
    )
    if value is None:
        value = default
    return value if value > 0 else None


def _stale_cleanup_message(
    violation: process_sentinel.SentinelViolation,
    *,
    killed_at: str,
) -> str:
    age = (
        "unknown"
        if violation.oldest_elapsed_sec is None
        else f"{violation.oldest_elapsed_sec:.0f}s"
    )
    stale_sec = (
        "unknown" if violation.stale_sec is None else f"{violation.stale_sec:.0f}s"
    )
    return (
        "memory_guard: stale orphaned Molt process group detected before "
        "guarded command; terminated it to prevent accumulated build/test "
        "processes: "
        f"killed_at={killed_at} pgid={violation.pgid} "
        f"age={age} threshold={stale_sec} reason={violation.reason} "
        f"pids={','.join(str(pid) for pid in violation.pids)} "
        f"command={violation.command}\n"
        "memory_guard: next action: inspect the matching sentinel JSONL event "
        "and prior logs; if the process was intentional, rerun it under an "
        "active suite sentinel or raise MOLT_STALE_ORPHAN_SEC.\n"
    )


def _repo_sentinel_repro_payload(
    *,
    command: str,
    cwd: str | Path | None,
    env: Mapping[str, str] | None,
    limits: HarnessMemoryLimits,
    resolved_limits: memory_guard.ResolvedMemoryLimits,
    label: str,
    accounted_rss_kb: int,
    timeout_s: float | None = None,
) -> dict[str, object]:
    command_payload = [command] if command else list(sys.argv)
    payload = memory_guard.repro_context_payload(
        command=command_payload,
        cwd=cwd,
        environ=_effective_env(env),
        max_process_rss_kb=resolved_limits.max_process_rss_kb,
        max_total_rss_kb=resolved_limits.max_total_rss_kb,
        max_global_rss_kb=resolved_limits.max_global_rss_kb,
        child_rlimit_kb=limits.current_child_rlimit_kb(
            env,
            accounted_rss_kb=accounted_rss_kb,
        ),
        timeout_s=timeout_s,
        poll_interval_s=limits.poll_interval,
        summary_json=None,
    )
    payload["sentinel_label"] = label
    return payload


def _prune_stale_repo_processes(
    *,
    prefix: str,
    env: Mapping[str, str] | None,
    limits: HarnessMemoryLimits,
) -> tuple[process_sentinel.SentinelViolation, ...]:
    if not _stale_orphan_cleanup_enabled(prefix, env):
        return ()
    stale_orphan_sec = _stale_seconds_from_env(
        prefix,
        env,
        suffix="STALE_ORPHAN_SEC",
        default=DEFAULT_STALE_ORPHAN_SEC,
    )
    stale_pytest_sec = _stale_seconds_from_env(
        prefix,
        env,
        suffix="STALE_PYTEST_SEC",
        default=DEFAULT_STALE_PYTEST_SEC,
    )
    if stale_orphan_sec is None and stale_pytest_sec is None:
        return ()
    samples = memory_guard.sample_processes()
    # CANONICAL: the preflight terminates ONLY under explicit guard custody, like
    # the continuous sentinel (commit 5df6b35d5 "Require explicit custody for repo
    # sentinel termination"). A guard about to launch a command owns nothing yet,
    # and repo-scope heuristics match parent shells, Codex/Claude helpers, and
    # unrelated processes that merely reference the repo path on their command
    # line (e.g. `powershell -Command "... python -m molt build <repo>..."`).
    # Signalling those repeatedly killed the operator's Codex CLI parents. With an
    # empty owned set there are ZERO kill candidates, so the preflight can never
    # terminate a process it cannot prove it owns. Cross-session cleanup is
    # operator-driven via `molt clean --kill-processes`.
    groups = process_sentinel.process_groups(
        samples,
        root=_REPO_ROOT,
        self_pid=os.getpid(),
        self_pgid=memory_guard._safe_getpgrp(),
        owned_pids=frozenset(),
    )
    accounted_rss_kb = sum(group.total_rss_kb for group in groups)
    current_limits = limits.current_memory_limits(
        env,
        accounted_rss_kb=accounted_rss_kb,
    )
    violations = process_sentinel.find_violations(
        groups,
        max_process_kb=sys.maxsize,
        max_group_kb=sys.maxsize,
        max_global_kb=sys.maxsize,
        stale_orphan_sec=stale_orphan_sec,
        stale_pytest_sec=stale_pytest_sec,
    )
    if not violations:
        return ()
    label = f"{_label_from_prefix(prefix)}_stale_preflight"
    events_path = _artifact_root_from_env(env) / "memory_guard" / f"{label}.jsonl"
    terminated: list[process_sentinel.SentinelViolation] = []
    for violation in violations:
        if not _claim_terminated_pgid(violation.pgid):
            continue
        killed_at = _utc_timestamp()
        _append_jsonl(
            events_path,
            {
                "event": "repo_process_guard_stale_preflight",
                "label": label,
                "violation": process_sentinel.violation_payload(violation),
                "repro": _repo_sentinel_repro_payload(
                    command=violation.command,
                    cwd=_REPO_ROOT,
                    env=env,
                    limits=limits,
                    resolved_limits=current_limits,
                    label=label,
                    accounted_rss_kb=accounted_rss_kb,
                ),
                "killed_at": killed_at,
                "kill_scope": "repo",
                "killer_label": label,
                "killer_pid": os.getpid(),
                "killer_session_id": os.environ.get("MOLT_SESSION_ID", ""),
                "victim_pgid": violation.pgid,
                "victim_command": violation.command,
                "owner_match_reason": "stale_orphan_repo_scope",
                "scope_to_current_tree": False,
                "claim_status": "claimed",
                "termination": {
                    "attempted": True,
                    "signal": memory_guard.term_signal_payload(),
                    "fallback_signal": memory_guard.fallback_kill_signal_payload(),
                    "grace_sec": 0.25,
                    "rss_triggered": False,
                },
                "action": (
                    "terminated stale orphaned repo-scoped Molt process group "
                    "before launching a guarded command"
                ),
            },
        )
        print(
            _stale_cleanup_message(violation, killed_at=killed_at),
            file=sys.stderr,
            end="",
        )
        process_sentinel.terminate_group(
            violation.pgid,
            grace=0.25,
            expected_identities=process_sentinel.process_group_expected_identities(
                violation
            ),
        )
        terminated.append(violation)
    return tuple(terminated)


@contextlib.contextmanager
def _auto_repo_sentinel(
    *,
    prefix: str,
    env: Mapping[str, str] | None,
    limits: HarnessMemoryLimits,
) -> Iterator[RepoProcessMemorySentinel | None]:
    if _sentinel_active() or _external_repo_sentinel_active(prefix, env):
        yield None
        return
    _prune_stale_repo_processes(prefix=prefix, env=env, limits=limits)
    label = f"{_label_from_prefix(prefix)}_command"
    with repo_process_sentinel(
        repo_root=_REPO_ROOT,
        artifact_root=_artifact_root_from_env(env),
        label=label,
        limits=limits,
        # Automatic command guards already own the direct child process tree via
        # memory_guard.run_guarded. Broad repo draining on context exit can
        # SIGTERM unrelated concurrent builds that appeared after the baseline.
        drain_on_exit=False,
        drain_until_clean_sec=0.1,
        drain_max_runtime_sec=2.0,
        suppress_auto_guard=False,
    ) as sentinel:
        yield sentinel


def guarded_completed_process(
    command: Sequence[str],
    *,
    prefix: str,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    input: str | None = None,
    capture_output: bool = True,
    text: bool = True,
    timeout: float | None = None,
    limits: HarnessMemoryLimits | None = None,
    stream: str = "",
    cleanup_orphans: bool | None = None,
    progress_label: str | None = None,
    encoding: str = "utf-8",
    errors: str = "replace",
) -> GuardedCompletedProcess:
    env = memory_guard.test_custody_launch_env(command, environ=env, cwd=cwd)
    resolved_limits = limits or limits_from_env(prefix, env)
    # Resolve a relative path-bearing interpreter against the parent cwd before
    # memory_guard.run_guarded hands it to the child spawn boundary.
    command = memory_guard._resolve_relative_executable(command)
    sentinel_is_active = _sentinel_active() or _external_repo_sentinel_active(
        prefix,
        env,
    )
    cleanup_tracked_orphans = (
        not sentinel_is_active if cleanup_orphans is None else cleanup_orphans
    )
    with _auto_repo_sentinel(
        prefix=prefix,
        env=env,
        limits=resolved_limits,
    ):
        default_progress_label = (
            f"memory_guard: {_normalize_prefix(prefix)} guarded command"
        )
        active_progress_label = (
            progress_label
            if progress_label is not None
            else (default_progress_label if not capture_output else None)
        )
        keepalive_interval = (
            _subprocess_keepalive_interval_secs(env, prefix=prefix)
            if active_progress_label is not None
            else None
        )
        guarded = memory_guard.run_guarded(
            list(command),
            max_rss_kb=resolved_limits.max_process_rss_kb,
            max_total_rss_kb=resolved_limits.max_total_rss_kb,
            poll_interval=resolved_limits.poll_interval,
            cwd=cwd,
            env=env,
            timeout=timeout,
            capture_output=capture_output,
            child_rlimit_kb=resolved_limits.current_child_rlimit_kb(env),
            input=input,
            stream=stream,
            text=text,
            adaptive_budget_provider=(
                lambda accounted: memory_guard.adaptive_memory_budget(
                    resolved_limits.adaptive_prefix,
                    _effective_env(env),
                    accounted_rss_kb=accounted,
                )
            ),
            dynamic_process_rss=resolved_limits.dynamic_process_rss,
            dynamic_total_rss=resolved_limits.dynamic_total_rss,
            cleanup_orphans=cleanup_tracked_orphans,
            progress_label=active_progress_label,
            keepalive_interval=keepalive_interval,
            encoding=encoding,
            errors=errors,
        )
    stderr: str | bytes = guarded.stderr or ("" if text else b"")
    incident_at = _utc_timestamp()
    if guarded.violation is not None:
        stderr = memory_guard._append_guard_message(
            stderr,
            _guard_stderr_message(
                guarded.violation,
                resolved_limits,
                guarded.limit_at_violation,
                prefix=prefix,
                elapsed_s=guarded.elapsed_s,
                killed_at=incident_at,
            ),
            text=text,
        )
        if guarded.guard_signal is not None:
            stderr = memory_guard._append_guard_message(
                stderr,
                _guard_parent_signal_message(
                    guarded.guard_signal,
                    elapsed_s=guarded.elapsed_s,
                    observed_at=incident_at,
                    primary_reason="rss_limit_exceeded",
                ),
                text=text,
            )
    elif guarded.timed_out:
        stderr = memory_guard._append_guard_message(
            stderr,
            _guard_timeout_message(
                prefix=prefix,
                timeout=timeout,
                elapsed_s=guarded.elapsed_s,
                killed_at=incident_at,
            ),
            text=text,
        )
        if guarded.guard_signal is not None:
            stderr = memory_guard._append_guard_message(
                stderr,
                _guard_parent_signal_message(
                    guarded.guard_signal,
                    elapsed_s=guarded.elapsed_s,
                    observed_at=incident_at,
                    primary_reason="timeout",
                ),
                text=text,
            )
    elif guarded.guard_signal is not None:
        stderr = memory_guard._append_guard_message(
            stderr,
            _guard_parent_signal_message(
                guarded.guard_signal,
                elapsed_s=guarded.elapsed_s,
                observed_at=incident_at,
            ),
            text=text,
        )
    else:
        stderr = memory_guard._append_guard_message(
            stderr,
            _guard_exit_signal_message(
                guarded.returncode,
                elapsed_s=guarded.elapsed_s,
                observed_at=incident_at,
            ),
            text=text,
        )
    if guarded.orphaned_process_groups:
        stderr = memory_guard._append_guard_message(
            stderr,
            _guard_orphan_cleanup_message(
                guarded.orphaned_process_groups,
                elapsed_s=guarded.elapsed_s,
                killed_at=incident_at,
            ),
            text=text,
        )
    if (
        guarded.violation is not None
        or guarded.timed_out
        or bool(guarded.orphaned_process_groups)
        or guarded.guard_signal is not None
        or memory_guard.exit_signal_payload(guarded.returncode) is not None
    ):
        stderr = memory_guard._append_guard_message(
            stderr,
            _guard_repro_message(
                command=command,
                cwd=cwd,
                env=env,
                limits=resolved_limits,
                timeout=timeout,
                prefix=prefix,
            ),
            text=text,
        )
    _profile_path, profile_error = _append_guarded_command_profile(
        command=command,
        prefix=prefix,
        cwd=cwd,
        env=env,
        limits=resolved_limits,
        returncode=guarded.returncode,
        elapsed_s=guarded.elapsed_s,
        timeout_s=timeout,
        violation=guarded.violation,
        timed_out=guarded.timed_out,
        limit_at_violation=guarded.limit_at_violation,
        orphaned_process_groups=guarded.orphaned_process_groups,
        peak=guarded.peak,
        peak_total=guarded.peak_total,
        cargo_incremental_quarantine=guarded.cargo_incremental_quarantine,
        child_process=guarded.child_process,
        termination_reports=guarded.termination_reports,
        guard_signal=guarded.guard_signal,
    )
    if profile_error:
        stderr = memory_guard._append_guard_message(stderr, profile_error, text=text)
    return GuardedCompletedProcess(
        list(command),
        guarded.returncode,
        guarded.stdout,
        stderr,
        elapsed_s=guarded.elapsed_s,
        violation=guarded.violation,
        timed_out=guarded.timed_out,
        limit_at_violation=guarded.limit_at_violation,
        orphaned_process_groups=guarded.orphaned_process_groups,
        cargo_incremental_quarantine=guarded.cargo_incremental_quarantine,
        child_process=guarded.child_process,
        termination_reports=guarded.termination_reports,
        guard_signal=guarded.guard_signal,
    )


def _subprocess_keepalive_interval_secs(
    env: Mapping[str, str] | None = None,
    *,
    prefix: str | None = None,
) -> float | None:
    source = _effective_env(env)
    normalized = _normalize_prefix(prefix or "")
    names: list[str] = []
    if normalized:
        names.extend(
            [
                f"{normalized}_KEEPALIVE_SEC",
                f"{normalized}_KEEPALIVE_SECS",
            ]
        )
    names.append("MOLT_SUBPROCESS_KEEPALIVE_SECS")
    raw = ""
    for name in names:
        value = source.get(name)
        if value is not None:
            raw = value.strip()
            break
    if not raw:
        raw = "20"
    if raw.lower() in {"0", "off", "false", "no"}:
        return None
    try:
        value = float(raw)
    except ValueError:
        return 20.0
    return value if value > 0 else None


def _guard_output_bytes(value: str | bytes | None) -> bytes:
    if value is None:
        return b""
    if isinstance(value, bytes):
        return value
    return value.encode("utf-8", errors="replace")


def _append_guard_bytes(stderr: bytes, message: str) -> bytes:
    return stderr + message.encode("utf-8", errors="replace")


def guarded_completed_process_to_tempfiles(
    command: Sequence[str],
    *,
    prefix: str,
    input: bytes | None = None,
    cwd: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    progress_label: str | None = None,
    limits: HarnessMemoryLimits | None = None,
) -> subprocess.CompletedProcess[bytes]:
    """Run a guarded command while capturing stdout/stderr through temp files.

    This preserves the shared memory-guard contract for commands whose
    descendants may inherit stdout/stderr and keep pipe handles open after the
    direct child exits.
    """

    resolved_limits = limits or limits_from_env(prefix, env)
    # Resolve a relative path-bearing executable against the parent cwd before
    # memory_guard.run_guarded hands it to the child spawn boundary.
    command = memory_guard._resolve_relative_executable(command)
    sentinel_is_active = _sentinel_active() or _external_repo_sentinel_active(
        prefix,
        env,
    )
    cleanup_tracked_orphans = not sentinel_is_active
    with _auto_repo_sentinel(
        prefix=prefix,
        env=env,
        limits=resolved_limits,
    ):
        keepalive_interval = (
            _subprocess_keepalive_interval_secs(env, prefix=prefix)
            if progress_label is not None
            else None
        )
        guarded = memory_guard.run_guarded(
            list(command),
            max_rss_kb=resolved_limits.max_process_rss_kb,
            max_total_rss_kb=resolved_limits.max_total_rss_kb,
            poll_interval=resolved_limits.poll_interval,
            cwd=cwd,
            env=env,
            timeout=timeout,
            capture_output=True,
            child_rlimit_kb=resolved_limits.current_child_rlimit_kb(env),
            input=input,
            adaptive_budget_provider=(
                lambda accounted: memory_guard.adaptive_memory_budget(
                    resolved_limits.adaptive_prefix,
                    _effective_env(env),
                    accounted_rss_kb=accounted,
                )
            ),
            dynamic_process_rss=resolved_limits.dynamic_process_rss,
            dynamic_total_rss=resolved_limits.dynamic_total_rss,
            cleanup_orphans=cleanup_tracked_orphans,
            progress_label=progress_label,
            keepalive_interval=keepalive_interval,
            text=False,
        )
    stdout = _guard_output_bytes(guarded.stdout)
    stderr = _guard_output_bytes(guarded.stderr)
    incident_at = _utc_timestamp()
    if guarded.violation is not None:
        stderr = _append_guard_bytes(
            stderr,
            _guard_stderr_message(
                guarded.violation,
                resolved_limits,
                guarded.limit_at_violation,
                prefix=prefix,
                elapsed_s=guarded.elapsed_s,
                killed_at=incident_at,
            ),
        )
        if guarded.guard_signal is not None:
            stderr = _append_guard_bytes(
                stderr,
                _guard_parent_signal_message(
                    guarded.guard_signal,
                    elapsed_s=guarded.elapsed_s,
                    observed_at=incident_at,
                    primary_reason="rss_limit_exceeded",
                ),
            )
    elif guarded.timed_out:
        stderr = _append_guard_bytes(
            stderr,
            _guard_timeout_message(
                prefix=prefix,
                timeout=timeout,
                elapsed_s=guarded.elapsed_s,
                killed_at=incident_at,
            ),
        )
        if guarded.guard_signal is not None:
            stderr = _append_guard_bytes(
                stderr,
                _guard_parent_signal_message(
                    guarded.guard_signal,
                    elapsed_s=guarded.elapsed_s,
                    observed_at=incident_at,
                    primary_reason="timeout",
                ),
            )
    elif guarded.guard_signal is not None:
        stderr = _append_guard_bytes(
            stderr,
            _guard_parent_signal_message(
                guarded.guard_signal,
                elapsed_s=guarded.elapsed_s,
                observed_at=incident_at,
            ),
        )
    else:
        stderr = _append_guard_bytes(
            stderr,
            _guard_exit_signal_message(
                guarded.returncode,
                elapsed_s=guarded.elapsed_s,
                observed_at=incident_at,
            ),
        )
    if guarded.orphaned_process_groups:
        stderr = _append_guard_bytes(
            stderr,
            _guard_orphan_cleanup_message(
                guarded.orphaned_process_groups,
                elapsed_s=guarded.elapsed_s,
                killed_at=incident_at,
            ),
        )
    if (
        guarded.violation is not None
        or guarded.timed_out
        or bool(guarded.orphaned_process_groups)
        or guarded.guard_signal is not None
        or memory_guard.exit_signal_payload(guarded.returncode) is not None
    ):
        stderr = _append_guard_bytes(
            stderr,
            _guard_repro_message(
                command=command,
                cwd=cwd,
                env=env,
                limits=resolved_limits,
                timeout=timeout,
                prefix=prefix,
            ),
        )
    _profile_path, profile_error = _append_guarded_command_profile(
        command=command,
        prefix=prefix,
        cwd=cwd,
        env=env,
        limits=resolved_limits,
        returncode=guarded.returncode,
        elapsed_s=guarded.elapsed_s,
        timeout_s=timeout,
        violation=guarded.violation,
        timed_out=guarded.timed_out,
        limit_at_violation=guarded.limit_at_violation,
        orphaned_process_groups=guarded.orphaned_process_groups,
        peak=guarded.peak,
        peak_total=guarded.peak_total,
        cargo_incremental_quarantine=guarded.cargo_incremental_quarantine,
        child_process=guarded.child_process,
        termination_reports=guarded.termination_reports,
        guard_signal=guarded.guard_signal,
    )
    if profile_error:
        stderr = _append_guard_bytes(stderr, profile_error)
    return GuardedCompletedProcess(
        list(command),
        guarded.returncode,
        stdout,
        stderr,
        elapsed_s=guarded.elapsed_s,
        violation=guarded.violation,
        timed_out=guarded.timed_out,
        limit_at_violation=guarded.limit_at_violation,
        orphaned_process_groups=guarded.orphaned_process_groups,
        cargo_incremental_quarantine=guarded.cargo_incremental_quarantine,
        child_process=guarded.child_process,
        termination_reports=guarded.termination_reports,
        guard_signal=guarded.guard_signal,
    )


def batch_process_group_kwargs(
    limits: HarnessMemoryLimits | None = None,
    *,
    env: Mapping[str, str] | None = None,
) -> dict[str, object]:
    resolved_limits = limits or limits_from_env("MOLT", env)
    if os.name == "nt":
        creationflags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        return {"creationflags": creationflags} if creationflags else {}
    if os.name != "posix":
        return {}
    kwargs: dict[str, object] = {"start_new_session": True}
    child_rlimit_kb = resolved_limits.current_child_rlimit_kb(env)
    if child_rlimit_kb is not None:
        kwargs["preexec_fn"] = _child_resource_limit_preexec(child_rlimit_kb)
    return kwargs


def _child_resource_limit_preexec(limit_kb: int) -> Callable[[], None]:
    def apply_limit() -> None:
        memory_guard._apply_child_resource_limit(limit_kb)

    return apply_limit


def force_close_process_group(proc: subprocess.Popen[str]) -> None:
    if proc.poll() is not None:
        return
    tracker = memory_guard.ProcessTreeTracker(proc.pid)
    samples = memory_guard.sample_processes()
    watched = tracker.update(samples)
    memory_guard.terminate_watched_processes(
        proc.pid,
        samples=samples,
        watched=watched,
        grace=0.25,
        root_owned=True,
    )
    deadline = time.monotonic() + 1.0
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            return
        time.sleep(0.05)
    samples = memory_guard.sample_processes()
    watched = tracker.update(samples)
    memory_guard.terminate_watched_processes(
        proc.pid,
        samples=samples,
        watched=watched,
        grace=0.0,
        root_owned=True,
    )
    with contextlib.suppress(subprocess.TimeoutExpired):
        proc.wait(timeout=0.5)


class RepoProcessMemorySentinel:
    def __init__(
        self,
        *,
        repo_root: Path,
        artifact_root: Path,
        label: str,
        limits: HarnessMemoryLimits,
        drain_on_exit: bool = True,
        drain_grace_sec: float = 0.25,
        drain_until_clean_sec: float = 0.3,
        drain_max_runtime_sec: float = 5.0,
        suppress_auto_guard: bool = True,
        scope_to_current_tree: bool = True,
        on_scan: Callable[
            [
                Sequence[process_sentinel.ProcessGroup],
                memory_guard.ResolvedMemoryLimits,
                float,
            ],
            None,
        ]
        | None = None,
        on_violation: Callable[
            [
                process_sentinel.SentinelViolation,
                memory_guard.ResolvedMemoryLimits,
                Mapping[str, object],
            ],
            None,
        ]
        | None = None,
    ) -> None:
        self._repo_root = repo_root
        self._artifact_root = artifact_root
        self._label = label
        self._limits = limits
        self._drain_on_exit = drain_on_exit
        self._drain_grace_sec = max(0.0, drain_grace_sec)
        self._drain_until_clean_sec = max(0.0, drain_until_clean_sec)
        self._drain_max_runtime_sec = max(0.0, drain_max_runtime_sec)
        self._suppress_auto_guard = suppress_auto_guard
        self._scope_to_current_tree = scope_to_current_tree
        self._on_scan = on_scan
        self._on_violation = on_violation
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._tree_tracker = memory_guard.ProcessTreeTracker(os.getpid())
        self._baseline_pgids: set[int] = set()
        self._observed_process_identities: dict[int, memory_guard.ProcessIdentity] = {}
        self._terminated_pgids: set[int] = set()
        self._protected_pgids_recorded: set[int] = set()
        self.tripped = False
        self._started_monotonic = time.monotonic()
        self._started_at = _utc_timestamp()
        self.events_path = artifact_root / "memory_guard" / f"{label}_sentinel.jsonl"

    def __enter__(self) -> "RepoProcessMemorySentinel":
        if self._suppress_auto_guard:
            _note_auto_sentinel_suppressor_entered()
        try:
            self._started_monotonic = time.monotonic()
            self._started_at = _utc_timestamp()
            self._baseline_pgids = self._current_group_pgids()
            self._thread = threading.Thread(
                target=self._run,
                name=f"{self._label}-memory-sentinel",
                daemon=True,
            )
            self._thread.start()
            return self
        except Exception:
            if self._suppress_auto_guard:
                _note_auto_sentinel_suppressor_exited()
            raise

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        try:
            self._stop.set()
            if self._thread is not None:
                self._thread.join(timeout=max(0.5, self._limits.poll_interval * 2))
            if self._drain_on_exit:
                self.drain_new_processes()
        finally:
            if self._suppress_auto_guard:
                _note_auto_sentinel_suppressor_exited()

    def _record(self, payload: dict[str, object]) -> None:
        payload.setdefault("label", self._label)
        payload.setdefault("ts", time.time())
        _append_jsonl(self.events_path, payload)

    def _elapsed_s(self) -> float:
        return max(0.0, time.monotonic() - self._started_monotonic)

    def _owned_pids_from_samples(
        self,
        samples: Mapping[int, memory_guard.ProcessSample],
    ) -> set[int]:
        if not self._scope_to_current_tree:
            return set()
        self._tree_tracker.update(samples)
        known_pids = set(self._tree_tracker.known_pids or set())
        return {pid for pid in known_pids if pid in samples}

    def _termination_attribution(
        self,
        *,
        victim_pgid: int,
        victim_command: str,
        grace_sec: float,
        rss_triggered: bool,
        attempted: bool,
        claim_status: str,
    ) -> dict[str, object]:
        kill_scope = "current-tree" if self._scope_to_current_tree else "repo"
        payload: dict[str, object] = {
            "kill_scope": kill_scope,
            "victim_pgid": victim_pgid,
            "victim_command": victim_command,
            "owner_match_reason": (
                "current_process_tree" if self._scope_to_current_tree else "repo_scope"
            ),
            "scope_to_current_tree": self._scope_to_current_tree,
            "claim_status": claim_status,
            "termination": {
                "attempted": attempted,
                "signal": memory_guard.term_signal_payload(),
                "fallback_signal": memory_guard.fallback_kill_signal_payload(),
                "grace_sec": grace_sec,
                "rss_triggered": rss_triggered,
            },
        }
        session_id = os.environ.get("MOLT_SESSION_ID", "")
        if attempted:
            payload.update(
                {
                    "killer_label": self._label,
                    "killer_pid": os.getpid(),
                    "killer_session_id": session_id,
                }
            )
        else:
            payload.update(
                {
                    "observer_label": self._label,
                    "observer_pid": os.getpid(),
                    "observer_session_id": session_id,
                }
            )
        return payload

    def _run(self) -> None:
        while not self._stop.wait(self._limits.poll_interval):
            self.scan_once()

    def _notify_scan(
        self,
        groups: Sequence[process_sentinel.ProcessGroup],
        limits: memory_guard.ResolvedMemoryLimits,
    ) -> None:
        if self._on_scan is None:
            return
        try:
            self._on_scan(groups, limits, self._elapsed_s())
        except Exception as exc:  # noqa: BLE001
            self._record(
                {
                    "event": "repo_process_guard_callback_error",
                    "callback": "on_scan",
                    "error": str(exc),
                }
            )

    def _notify_violation(
        self,
        violation: process_sentinel.SentinelViolation,
        limits: memory_guard.ResolvedMemoryLimits,
        payload: Mapping[str, object],
    ) -> None:
        if self._on_violation is None:
            return
        try:
            self._on_violation(violation, limits, payload)
        except Exception as exc:  # noqa: BLE001
            self._record(
                {
                    "event": "repo_process_guard_callback_error",
                    "callback": "on_violation",
                    "error": str(exc),
                }
            )

    def _current_groups(
        self,
        *,
        update_observed: bool = True,
    ) -> list[process_sentinel.ProcessGroup]:
        samples = memory_guard.sample_processes()
        self._record_skipped_protected_groups(samples)
        owned_pids = self._owned_pids_from_samples(samples)
        known_process_identities = dict(self._observed_process_identities)
        known_process_identities.update(
            {
                pid: memory_guard.process_identity(samples[pid])
                for pid in owned_pids
                if pid in samples
            }
        )
        groups = process_sentinel.process_groups(
            samples,
            root=self._repo_root,
            self_pid=os.getpid(),
            self_pgid=memory_guard._safe_getpgrp(),
            known_process_identities=known_process_identities,
            owned_pids=owned_pids if self._scope_to_current_tree else None,
        )
        if update_observed:
            for group in groups:
                for sample in group.samples:
                    self._observed_process_identities[sample.pid] = (
                        memory_guard.process_identity(sample)
                    )
        return groups

    def _record_skipped_protected_groups(
        self,
        samples: Mapping[int, memory_guard.ProcessSample],
    ) -> None:
        protected = process_sentinel.skipped_protected_process_groups(
            samples,
            root=self._repo_root,
            self_pid=os.getpid(),
            self_pgid=memory_guard._safe_getpgrp(),
            known_process_identities=self._observed_process_identities,
        )
        for group in protected:
            if not any(
                process_sentinel.is_host_control_plane_process(sample)
                for sample in group.samples
            ):
                continue
            if group.pgid in self._protected_pgids_recorded:
                continue
            self._protected_pgids_recorded.add(group.pgid)
            peak = group.peak
            self._record(
                {
                    "event": "repo_process_guard_protected_host_group",
                    "pgid": group.pgid,
                    "pids": group.pids,
                    "command": "" if peak is None else peak.command,
                    "guard_started_at": self._started_at,
                    "observed_at": _utc_timestamp(),
                    "elapsed_s": self._elapsed_s(),
                    "action": (
                        "excluded protected host/control-plane process group from "
                        "Molt repo process guard kill set"
                    ),
                }
            )

    def _current_group_pgids(self) -> set[int]:
        try:
            return {group.pgid for group in self._current_groups(update_observed=False)}
        except Exception as exc:  # noqa: BLE001
            self._record(
                {
                    "event": "repo_process_guard_baseline_error",
                    "error": str(exc),
                }
            )
            return set()

    def scan_once(self) -> None:
        try:
            groups = self._current_groups()
            accounted_rss_kb = sum(group.total_rss_kb for group in groups)
            current_limits = self._limits.current_memory_limits(
                accounted_rss_kb=accounted_rss_kb,
            )
            self._notify_scan(groups, current_limits)
            violations = process_sentinel.find_violations(
                groups,
                max_process_kb=current_limits.max_process_rss_kb,
                max_group_kb=current_limits.max_total_rss_kb
                if current_limits.max_total_rss_kb is not None
                else self._limits.max_total_rss_kb,
                max_global_kb=current_limits.max_global_rss_kb
                if current_limits.max_global_rss_kb is not None
                else self._limits.max_global_rss_kb,
            )
            if not violations:
                return
            self.tripped = True
            global_total_kb = sum(group.total_rss_kb for group in groups)
            active_pgids = [group.pgid for group in groups]
            for violation in violations:
                claimed = False
                if violation.pgid not in self._terminated_pgids:
                    claimed = _claim_terminated_pgid(violation.pgid)
                claim_status = "claimed" if claimed else "already_claimed"
                observed_at = _utc_timestamp()
                if claimed:
                    action = (
                        "terminated process group to prevent orphaned Molt "
                        "subprocesses; inspect this JSONL event, child logs, and "
                        "guard limits before rerun"
                    )
                else:
                    action = (
                        "process group was already claimed by another guard; inspect "
                        "the first matching guard event for kill details"
                    )
                payload = {
                    "event": "repo_process_guard_tripped",
                    "violation": process_sentinel.violation_payload(violation),
                    "limits": memory_guard.memory_limits_payload(current_limits),
                    "guard_started_at": self._started_at,
                    "observed_at": observed_at,
                    "elapsed_s": self._elapsed_s(),
                    "global_total_kb": global_total_kb,
                    "global_total_gb": global_total_kb / (1024 * 1024),
                    "active_pgids": active_pgids,
                    "repro": _repo_sentinel_repro_payload(
                        command=violation.command,
                        cwd=self._repo_root,
                        env=None,
                        limits=self._limits,
                        resolved_limits=current_limits,
                        label=self._label,
                        accounted_rss_kb=accounted_rss_kb,
                    ),
                    **self._termination_attribution(
                        victim_pgid=violation.pgid,
                        victim_command=violation.command,
                        grace_sec=0.25,
                        rss_triggered=True,
                        attempted=claimed,
                        claim_status=claim_status,
                    ),
                    "action": action,
                }
                if claimed:
                    payload["killed_at"] = observed_at
                self._record(payload)
                self._notify_violation(violation, current_limits, payload)
                if not claimed:
                    continue
                process_sentinel.terminate_group(
                    violation.pgid,
                    grace=0.25,
                    expected_identities=process_sentinel.process_group_expected_identities(
                        violation
                    ),
                )
                self._terminated_pgids.add(violation.pgid)
        except Exception as exc:  # noqa: BLE001
            self._record(
                {
                    "event": "repo_process_guard_error",
                    "error": str(exc),
                }
            )

    def _new_groups(self) -> list[process_sentinel.ProcessGroup]:
        return [
            group
            for group in self._current_groups()
            if group.pgid not in self._baseline_pgids
        ]

    def drain_new_processes(self) -> int:
        drained = 0
        drained_pgids: set[int] = set()
        clean_since: float | None = None
        started = time.monotonic()
        while True:
            groups = self._new_groups()
            now = time.monotonic()
            if not groups:
                if self._drain_until_clean_sec <= 0:
                    return drained
                if clean_since is None:
                    clean_since = now
                if now - clean_since >= self._drain_until_clean_sec:
                    return drained
            else:
                clean_since = None
                accounted_rss_kb = sum(group.total_rss_kb for group in groups)
                current_limits = self._limits.current_memory_limits(
                    accounted_rss_kb=accounted_rss_kb,
                )
                for group in groups:
                    if (
                        group.pgid in drained_pgids
                        or group.pgid in self._terminated_pgids
                        or not _claim_terminated_pgid(group.pgid)
                    ):
                        continue
                    peak = group.peak
                    violation = process_sentinel.SentinelViolation(
                        pgid=group.pgid,
                        reason="drain_on_exit",
                        total_rss_kb=group.total_rss_kb,
                        peak_pid=None if peak is None else peak.pid,
                        peak_rss_kb=None if peak is None else peak.rss_kb,
                        pids=tuple(group.pids),
                        command="" if peak is None else peak.command,
                        samples=group.samples,
                        external_parent_pids=tuple(group.external_parent_pids),
                        oldest_elapsed_sec=group.oldest_elapsed_sec,
                        orphaned=group.is_orphaned,
                    )
                    self._record(
                        {
                            "event": "repo_process_guard_drained",
                            "violation": process_sentinel.violation_payload(violation),
                            "guard_started_at": self._started_at,
                            "killed_at": _utc_timestamp(),
                            "elapsed_s": self._elapsed_s(),
                            "repro": _repo_sentinel_repro_payload(
                                command=violation.command,
                                cwd=self._repo_root,
                                env=None,
                                limits=self._limits,
                                resolved_limits=current_limits,
                                label=self._label,
                                accounted_rss_kb=accounted_rss_kb,
                                timeout_s=self._drain_max_runtime_sec,
                            ),
                            **self._termination_attribution(
                                victim_pgid=violation.pgid,
                                victim_command=violation.command,
                                grace_sec=self._drain_grace_sec,
                                rss_triggered=False,
                                attempted=True,
                                claim_status="claimed",
                            ),
                            "action": (
                                "terminated process group left behind by the guarded "
                                "scope to prevent orphaned Molt subprocesses; inspect "
                                "child logs before rerun"
                            ),
                        }
                    )
                    process_sentinel.terminate_group(
                        group.pgid,
                        grace=self._drain_grace_sec,
                        expected_identities={
                            sample.pid: memory_guard.process_identity(sample)
                            for sample in group.samples
                        },
                    )
                    drained_pgids.add(group.pgid)
                    self._terminated_pgids.add(group.pgid)
                    drained += 1
            if (
                self._drain_max_runtime_sec > 0
                and now - started >= self._drain_max_runtime_sec
            ):
                if groups:
                    self._record(
                        {
                            "event": "repo_process_guard_drain_timeout",
                            "remaining_pgids": [group.pgid for group in groups],
                            "guard_started_at": self._started_at,
                            "observed_at": _utc_timestamp(),
                            "elapsed_s": self._elapsed_s(),
                            "action": (
                                "drain did not reach a clean process table before "
                                "its bounded timeout; inspect remaining process "
                                "groups and either stop them or raise the drain "
                                "window for this suite"
                            ),
                        }
                    )
                return drained
            time.sleep(self._limits.poll_interval)


def repo_process_sentinel(
    *,
    repo_root: Path,
    artifact_root: Path,
    label: str,
    limits: HarnessMemoryLimits | None = None,
    drain_on_exit: bool = True,
    drain_grace_sec: float = 0.25,
    drain_until_clean_sec: float = 0.3,
    drain_max_runtime_sec: float = 5.0,
    suppress_auto_guard: bool = True,
    scope_to_current_tree: bool = True,
    on_scan: Callable[
        [
            Sequence[process_sentinel.ProcessGroup],
            memory_guard.ResolvedMemoryLimits,
            float,
        ],
        None,
    ]
    | None = None,
    on_violation: Callable[
        [
            process_sentinel.SentinelViolation,
            memory_guard.ResolvedMemoryLimits,
            Mapping[str, object],
        ],
        None,
    ]
    | None = None,
) -> RepoProcessMemorySentinel:
    return RepoProcessMemorySentinel(
        repo_root=repo_root,
        artifact_root=artifact_root,
        label=label,
        limits=limits or limits_from_env("MOLT"),
        drain_on_exit=drain_on_exit,
        drain_grace_sec=drain_grace_sec,
        drain_until_clean_sec=drain_until_clean_sec,
        drain_max_runtime_sec=drain_max_runtime_sec,
        suppress_auto_guard=suppress_auto_guard,
        scope_to_current_tree=scope_to_current_tree,
        on_scan=on_scan,
        on_violation=on_violation,
    )


@dataclass(frozen=True, slots=True)
class HarnessExecutionContext:
    prefix: str
    repo_root: Path
    env: Mapping[str, str]
    limits: HarnessMemoryLimits
    artifact_root: Path

    @classmethod
    def from_env(
        cls,
        prefix: str,
        env: Mapping[str, str] | None = None,
        *,
        repo_root: Path | None = None,
        artifact_root: Path | None = None,
        limits: HarnessMemoryLimits | None = None,
    ) -> "HarnessExecutionContext":
        root = (repo_root or _REPO_ROOT).resolve()
        canonical_env = canonical_harness_env(env, repo_root=root)
        resolved_limits = limits or limits_from_env(prefix, canonical_env)
        resolved_artifact_root = artifact_root or _artifact_root_from_env(canonical_env)
        return cls(
            prefix=_normalize_prefix(prefix),
            repo_root=root,
            env=canonical_env,
            limits=resolved_limits,
            artifact_root=resolved_artifact_root,
        )

    @property
    def memory_guard(self) -> dict[str, object]:
        return limits_summary(self.limits)

    def run(
        self,
        command: Sequence[str],
        *,
        cwd: str | Path | None = None,
        env: Mapping[str, str] | None = None,
        input: str | None = None,
        capture_output: bool = True,
        text: bool = True,
        timeout: float | None = None,
        stream: str = "",
        progress_label: str | None = None,
        encoding: str = "utf-8",
        errors: str = "replace",
    ) -> GuardedCompletedProcess:
        command_env = (
            self.env
            if env is None
            else canonical_harness_env(
                env,
                repo_root=self.repo_root,
            )
        )
        limits = (
            self.limits
            if env is None
            else limits_from_env(
                self.prefix,
                command_env,
            )
        )
        return guarded_completed_process(
            command,
            prefix=self.prefix,
            cwd=cwd,
            env=command_env,
            input=input,
            capture_output=capture_output,
            text=text,
            timeout=timeout,
            limits=limits,
            stream=stream,
            progress_label=progress_label,
            encoding=encoding,
            errors=errors,
        )

    def process_group_kwargs(self) -> dict[str, object]:
        return batch_process_group_kwargs(self.limits, env=self.env)

    def force_close_process_group(self, proc: subprocess.Popen[str]) -> None:
        force_close_process_group(proc)

    def start_repo_sentinel(
        self,
        *,
        label: str,
        drain_on_exit: bool = True,
        drain_grace_sec: float = 0.25,
        drain_until_clean_sec: float = 0.3,
        drain_max_runtime_sec: float = 5.0,
        suppress_auto_guard: bool = True,
        on_scan: Callable[
            [
                Sequence[process_sentinel.ProcessGroup],
                memory_guard.ResolvedMemoryLimits,
                float,
            ],
            None,
        ]
        | None = None,
        on_violation: Callable[
            [
                process_sentinel.SentinelViolation,
                memory_guard.ResolvedMemoryLimits,
                Mapping[str, object],
            ],
            None,
        ]
        | None = None,
    ) -> RepoProcessMemorySentinel | None:
        if _sentinel_active() or _external_repo_sentinel_active(self.prefix, self.env):
            return None
        sentinel = repo_process_sentinel(
            repo_root=self.repo_root,
            artifact_root=self.artifact_root,
            label=label,
            limits=self.limits,
            drain_on_exit=drain_on_exit,
            drain_grace_sec=drain_grace_sec,
            drain_until_clean_sec=drain_until_clean_sec,
            drain_max_runtime_sec=drain_max_runtime_sec,
            suppress_auto_guard=suppress_auto_guard,
            on_scan=on_scan,
            on_violation=on_violation,
        )
        sentinel.__enter__()
        return sentinel


@dataclass(frozen=True, slots=True)
class HarnessGuardScope:
    limits: HarnessMemoryLimits
    sentinel: RepoProcessMemorySentinel

    @property
    def memory_guard(self) -> dict[str, object]:
        return limits_summary(self.limits)


@contextlib.contextmanager
def guarded_harness_scope(
    *,
    prefix: str,
    repo_root: Path,
    artifact_root: Path,
    label: str,
    env: Mapping[str, str] | None = None,
    limits: HarnessMemoryLimits | None = None,
    drain_on_exit: bool = True,
    drain_grace_sec: float = 0.25,
    drain_until_clean_sec: float = 0.3,
    drain_max_runtime_sec: float = 5.0,
    on_scan: Callable[
        [
            Sequence[process_sentinel.ProcessGroup],
            memory_guard.ResolvedMemoryLimits,
            float,
        ],
        None,
    ]
    | None = None,
    on_violation: Callable[
        [
            process_sentinel.SentinelViolation,
            memory_guard.ResolvedMemoryLimits,
            Mapping[str, object],
        ],
        None,
    ]
    | None = None,
) -> Iterator[HarnessGuardScope]:
    resolved_limits = limits or limits_from_env(prefix, env)
    with repo_process_sentinel(
        repo_root=repo_root,
        artifact_root=artifact_root,
        label=label,
        limits=resolved_limits,
        drain_on_exit=drain_on_exit,
        drain_grace_sec=drain_grace_sec,
        drain_until_clean_sec=drain_until_clean_sec,
        drain_max_runtime_sec=drain_max_runtime_sec,
        on_scan=on_scan,
        on_violation=on_violation,
    ) as sentinel:
        yield HarnessGuardScope(limits=resolved_limits, sentinel=sentinel)
