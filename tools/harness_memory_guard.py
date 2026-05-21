#!/usr/bin/env python3
from __future__ import annotations

import contextlib
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Callable, Iterator, Mapping, Sequence

try:
    from tools import memory_guard, process_sentinel
except ModuleNotFoundError:  # pragma: no cover - direct script import from tools/
    import memory_guard  # type: ignore
    import process_sentinel  # type: ignore


DEFAULT_POLL_INTERVAL_SEC = 0.10
TRUE_VALUES = {"1", "true", "yes", "on"}
FALSE_VALUES = {"0", "false", "no", "off"}
TERMINATED_PGID_TTL_SEC = 60.0
HARD_RSS_LIMIT_GB = memory_guard.DEFAULT_HARD_MAX_RSS_GB - 0.001
HARD_GLOBAL_RSS_LIMIT_GB = memory_guard.DEFAULT_HARD_MAX_GLOBAL_RSS_GB - 0.001
HARD_CHILD_RLIMIT_GB = memory_guard.DEFAULT_HARD_MAX_CHILD_RLIMIT_GB - 0.001
_REPO_ROOT = Path(__file__).resolve().parents[1]
_TERMINATED_PGIDS: dict[int, float] = {}
_TERMINATED_PGIDS_LOCK = threading.Lock()
_AUTO_SENTINEL_SUPPRESSORS = 0
_AUTO_SENTINEL_SUPPRESSORS_LOCK = threading.Lock()


class GuardedCompletedProcess(subprocess.CompletedProcess[str]):
    def __init__(
        self,
        args: Sequence[str],
        returncode: int,
        stdout: str | None,
        stderr: str | None,
        *,
        elapsed_s: float | None,
    ) -> None:
        super().__init__(
            args=list(args), returncode=returncode, stdout=stdout, stderr=stderr
        )
        self.elapsed_s = elapsed_s


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
    max_process_rss_kb: int = field(init=False, repr=False)
    max_total_rss_kb: int = field(init=False, repr=False)
    max_global_rss_kb: int = field(init=False, repr=False)
    child_rlimit_kb: int | None = field(init=False, repr=False)

    def __post_init__(self) -> None:
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
) -> dict[str, str]:
    """Return a subprocess env with repo-local artifact/cache defaults installed."""

    root = (repo_root or _REPO_ROOT).resolve()
    merged = dict(os.environ) if env is None else dict(env)
    ext_root = Path(merged.get("MOLT_EXT_ROOT", str(root))).expanduser()
    if not ext_root.is_absolute():
        ext_root = root / ext_root
    ext_root = ext_root.resolve()
    merged.setdefault("MOLT_EXT_ROOT", str(ext_root))
    merged.setdefault("CARGO_TARGET_DIR", str(ext_root / "target"))
    merged.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", merged["CARGO_TARGET_DIR"])
    merged.setdefault("MOLT_CACHE", str(ext_root / ".molt_cache"))
    merged.setdefault("MOLT_DIFF_ROOT", str(ext_root / "tmp" / "diff"))
    merged.setdefault("MOLT_DIFF_TMPDIR", str(ext_root / "tmp"))
    merged.setdefault("UV_CACHE_DIR", str(ext_root / ".uv-cache"))
    merged.setdefault("TMPDIR", str(ext_root / "tmp"))
    return merged


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


def enabled_from_env(
    prefix: str,
    env: Mapping[str, str] | None = None,
) -> bool:
    source = _effective_env(env)
    normalized = _normalize_prefix(prefix)
    return _env_bool(
        source,
        [f"{normalized}_MEMORY_GUARD", "MOLT_MEMORY_GUARD"],
        default=True,
    )


def limits_from_env(
    prefix: str,
    env: Mapping[str, str] | None = None,
) -> HarnessMemoryLimits:
    source = _effective_env(env)
    normalized = _normalize_prefix(prefix)
    adaptive_budget = memory_guard.adaptive_memory_budget(normalized, source)
    enabled = enabled_from_env(normalized, source)
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
        child_rlimit_cap_gb = memory_guard.default_child_rlimit_gb(
            max_process_rss_gb=process_gb,
            max_total_rss_gb=total_gb,
            max_global_rss_gb=global_gb,
        )
        child_rlimit_gb = _clamp_hard_limit(
            child_rlimit_gb,
            min(HARD_CHILD_RLIMIT_GB, child_rlimit_cap_gb),
        )
    return HarnessMemoryLimits(
        enabled=enabled,
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
    }


def limits_status_line(limits: HarnessMemoryLimits) -> str:
    return (
        "Memory guard: "
        f"enabled={limits.enabled} "
        f"process={limits.max_process_rss_gb:.2f}GB "
        f"tree={limits.max_total_rss_gb:.2f}GB "
        f"global={limits.max_global_rss_gb:.2f}GB "
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


def _append_jsonl(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(payload, sort_keys=True) + "\n")


def _guard_stderr_message(
    violation: memory_guard.RssViolation | None,
    limits: HarnessMemoryLimits,
    effective_limits: memory_guard.ResolvedMemoryLimits | None = None,
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
    return (
        "memory_guard: RSS limit exceeded: "
        f"pid={violation.pid} rss={violation.rss_gb:.2f}GB "
        f"limit={limit_gb:.2f}GB scope={violation.scope} "
        f"command={violation.command}\n"
    )


def _guard_exit_signal_message(returncode: int) -> str:
    payload = memory_guard.exit_signal_payload(returncode)
    if payload is None:
        return ""
    signame = payload["name"] or f"signal {payload['signal']}"
    return (
        "memory_guard: command exited with "
        f"{signame} status ({returncode}); no RSS violation observed\n"
    )


@contextlib.contextmanager
def _auto_repo_sentinel(
    *,
    prefix: str,
    env: Mapping[str, str] | None,
    limits: HarnessMemoryLimits,
) -> Iterator[RepoProcessMemorySentinel | None]:
    if not limits.enabled or _sentinel_active():
        yield None
        return
    label = f"{_label_from_prefix(prefix)}_command"
    with repo_process_sentinel(
        repo_root=_REPO_ROOT,
        artifact_root=_artifact_root_from_env(env),
        label=label,
        limits=limits,
        drain_on_exit=True,
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
) -> GuardedCompletedProcess:
    resolved_limits = limits or limits_from_env(prefix, env)
    if not resolved_limits.enabled:
        started = time.perf_counter()
        completed = subprocess.run(
            list(command),
            cwd=cwd,
            env=dict(env) if env is not None else None,
            input=input,
            capture_output=capture_output,
            text=text,
            timeout=timeout,
            check=False,
        )
        return GuardedCompletedProcess(
            list(command),
            completed.returncode,
            completed.stdout,
            completed.stderr,
            elapsed_s=time.perf_counter() - started,
        )
    with _auto_repo_sentinel(
        prefix=prefix,
        env=env,
        limits=resolved_limits,
    ):
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
            adaptive_budget_provider=(
                lambda accounted: memory_guard.adaptive_memory_budget(
                    resolved_limits.adaptive_prefix,
                    _effective_env(env),
                    accounted_rss_kb=accounted,
                )
            ),
            dynamic_process_rss=resolved_limits.dynamic_process_rss,
            dynamic_total_rss=resolved_limits.dynamic_total_rss,
        )
    stderr = guarded.stderr or ""
    if guarded.violation is not None:
        stderr += _guard_stderr_message(
            guarded.violation,
            resolved_limits,
            guarded.limit_at_violation,
        )
    elif not guarded.timed_out:
        stderr += _guard_exit_signal_message(guarded.returncode)
    return GuardedCompletedProcess(
        list(command),
        guarded.returncode,
        guarded.stdout,
        stderr,
        elapsed_s=guarded.elapsed_s,
    )


def _guard_violation_bytes_message(
    violation: memory_guard.RssViolation,
    limit_gb: float | None,
) -> bytes:
    rss_gb = violation.rss_kb / (1024 * 1024)
    scope = getattr(violation, "scope", "process")
    limit = "unknown" if limit_gb is None else f"{limit_gb:.2f}GB"
    command = str(getattr(violation, "command", "")).strip()
    message = (
        "\n"
        f"molt memory guard: RSS limit exceeded scope={scope} "
        f"pid={violation.pid} rss={rss_gb:.2f}GB limit={limit}"
        + (f" command={command}" if command else "")
        + "\n"
    )
    return message.encode("utf-8", errors="replace")


def _subprocess_keepalive_interval_secs() -> float | None:
    raw = os.environ.get("MOLT_SUBPROCESS_KEEPALIVE_SECS", "20").strip()
    if raw in {"", "0", "off", "false"}:
        return None
    try:
        value = float(raw)
    except ValueError:
        return 20.0
    return value if value > 0 else None


def _terminate_guarded_bytes_process(
    proc: subprocess.Popen[bytes],
    tracker: memory_guard.ProcessTreeTracker | None,
    *,
    grace: float,
) -> None:
    if tracker is None:
        proc.kill()
        return
    samples = memory_guard.sample_processes()
    watched = tracker.update(samples)
    memory_guard.terminate_watched_processes(
        proc.pid,
        samples=samples,
        watched=watched,
        grace=grace,
    )


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
    guard_enabled = bool(resolved_limits.enabled)
    popen_kwargs: dict[str, object] = {}
    if guard_enabled:
        popen_kwargs.update(batch_process_group_kwargs(resolved_limits, env=env))

    sentinel_scope = (
        _auto_repo_sentinel(prefix=prefix, env=env, limits=resolved_limits)
        if guard_enabled
        else contextlib.nullcontext(None)
    )
    with sentinel_scope:
        with (
            tempfile.TemporaryFile() as stdout_file,
            tempfile.TemporaryFile() as stderr_file,
        ):
            proc = subprocess.Popen(
                list(command),
                stdout=stdout_file,
                stderr=stderr_file,
                cwd=cwd,
                env=dict(env) if env is not None else None,
                stdin=subprocess.PIPE if input is not None else None,
                **popen_kwargs,
            )
            tracker = (
                memory_guard.ProcessTreeTracker(proc.pid) if guard_enabled else None
            )
            if input is not None and proc.stdin is not None:
                try:
                    proc.stdin.write(input)
                finally:
                    proc.stdin.close()
            keepalive_interval = (
                _subprocess_keepalive_interval_secs()
                if progress_label is not None
                else None
            )
            started = time.monotonic()
            next_keepalive = (
                started + keepalive_interval if keepalive_interval is not None else None
            )
            next_guard_sample = (
                started + max(0.01, resolved_limits.poll_interval)
                if guard_enabled
                else None
            )
            while True:
                now = time.monotonic()
                remaining = None if timeout is None else timeout - (now - started)
                if remaining is not None and remaining <= 0:
                    _terminate_guarded_bytes_process(proc, tracker, grace=0.0)
                    proc.wait()
                    assert timeout is not None
                    raise subprocess.TimeoutExpired(list(command), timeout)
                wait_timeout = remaining
                if next_keepalive is not None:
                    keepalive_wait = max(0.0, next_keepalive - now)
                    wait_timeout = (
                        keepalive_wait
                        if wait_timeout is None
                        else min(wait_timeout, keepalive_wait)
                    )
                if next_guard_sample is not None:
                    guard_wait = max(0.0, next_guard_sample - now)
                    wait_timeout = (
                        guard_wait
                        if wait_timeout is None
                        else min(wait_timeout, guard_wait)
                    )
                try:
                    returncode = proc.wait(timeout=wait_timeout)
                    break
                except subprocess.TimeoutExpired:
                    now = time.monotonic()
                    if next_guard_sample is not None and now >= next_guard_sample:
                        assert tracker is not None
                        samples = memory_guard.sample_processes()
                        watched = tracker.update(samples)
                        observed_total = memory_guard.total_rss(
                            samples,
                            root_pid=proc.pid,
                            watched=watched,
                        )
                        current_limits = resolved_limits.current_memory_limits(
                            env,
                            accounted_rss_kb=(
                                0 if observed_total is None else observed_total.rss_kb
                            ),
                        )
                        violation = memory_guard.find_rss_violation(
                            samples,
                            root_pid=proc.pid,
                            max_rss_kb=current_limits.max_process_rss_kb,
                            max_total_rss_kb=current_limits.max_total_rss_kb,
                            watched=watched,
                        )
                        if violation is not None:
                            limit_gb = (
                                current_limits.max_total_rss_gb
                                if getattr(violation, "scope", "") == "process_tree"
                                else current_limits.max_process_rss_gb
                            )
                            stderr_file.write(
                                _guard_violation_bytes_message(violation, limit_gb)
                            )
                            stderr_file.flush()
                            _terminate_guarded_bytes_process(
                                proc,
                                tracker,
                                grace=0.25,
                            )
                            with contextlib.suppress(Exception):
                                proc.wait(timeout=1.0)
                            returncode = memory_guard.GUARD_RETURN_CODE
                            break
                        next_guard_sample = now + max(
                            0.01,
                            resolved_limits.poll_interval,
                        )
                        continue
                    if next_keepalive is not None and now >= next_keepalive:
                        assert keepalive_interval is not None
                        elapsed = now - started
                        print(
                            f"{progress_label} still running... ({elapsed:.0f}s)",
                            file=sys.stderr,
                        )
                        next_keepalive = now + keepalive_interval
                        continue
                    if timeout is not None and now - started >= timeout:
                        _terminate_guarded_bytes_process(proc, tracker, grace=0.0)
                        proc.wait()
                        raise subprocess.TimeoutExpired(list(command), timeout)
            stdout_file.seek(0)
            stderr_file.seek(0)
            stdout = stdout_file.read()
            stderr = stderr_file.read()
    return subprocess.CompletedProcess(list(command), returncode, stdout, stderr)


def batch_process_group_kwargs(
    limits: HarnessMemoryLimits | None = None,
    *,
    env: Mapping[str, str] | None = None,
) -> dict[str, object]:
    resolved_limits = limits or limits_from_env("MOLT", env)
    if not resolved_limits.enabled or os.name != "posix":
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
    if os.name == "posix":
        tracker = memory_guard.ProcessTreeTracker(proc.pid)
        samples = memory_guard.sample_processes()
        watched = tracker.update(samples)
        memory_guard.terminate_watched_processes(
            proc.pid,
            samples=samples,
            watched=watched,
            grace=0.25,
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
        )
        with contextlib.suppress(subprocess.TimeoutExpired):
            proc.wait(timeout=0.5)
        return
    with contextlib.suppress(ProcessLookupError, OSError):
        proc.terminate()
    with contextlib.suppress(subprocess.TimeoutExpired):
        proc.wait(timeout=0.5)
    if proc.poll() is None:
        with contextlib.suppress(ProcessLookupError, OSError):
            proc.kill()


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
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._baseline_pgids: set[int] = set()
        self._observed_pgids: set[int] = set()
        self._terminated_pgids: set[int] = set()
        self.tripped = False
        self.events_path = artifact_root / "memory_guard" / f"{label}_sentinel.jsonl"

    def __enter__(self) -> "RepoProcessMemorySentinel":
        if not self._limits.enabled:
            return self
        if self._suppress_auto_guard:
            _note_auto_sentinel_suppressor_entered()
        try:
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
            if self._limits.enabled and self._drain_on_exit:
                self.drain_new_processes()
        finally:
            if self._limits.enabled and self._suppress_auto_guard:
                _note_auto_sentinel_suppressor_exited()

    def _record(self, payload: dict[str, object]) -> None:
        payload.setdefault("label", self._label)
        payload.setdefault("ts", time.time())
        _append_jsonl(self.events_path, payload)

    def _run(self) -> None:
        while not self._stop.wait(self._limits.poll_interval):
            self.scan_once()

    def _current_groups(
        self,
        *,
        update_observed: bool = True,
    ) -> list[process_sentinel.ProcessGroup]:
        groups = process_sentinel.process_groups(
            memory_guard.sample_processes(),
            root=self._repo_root,
            self_pid=os.getpid(),
            self_pgid=os.getpgrp(),
            known_pgids=self._observed_pgids,
        )
        if update_observed:
            self._observed_pgids.update(group.pgid for group in groups)
        return groups

    def _current_group_pgids(self) -> set[int]:
        try:
            return {
                group.pgid
                for group in self._current_groups(update_observed=False)
            }
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
            current_limits = self._limits.current_memory_limits(
                accounted_rss_kb=sum(group.total_rss_kb for group in groups),
            )
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
            for violation in violations:
                self._record(
                    {
                        "event": "repo_process_guard_tripped",
                        "violation": process_sentinel.violation_payload(violation),
                        "limits": memory_guard.memory_limits_payload(current_limits),
                    }
                )
                if (
                    violation.pgid in self._terminated_pgids
                    or not _claim_terminated_pgid(violation.pgid)
                ):
                    continue
                process_sentinel.terminate_group(violation.pgid, grace=0.25)
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
                    )
                    self._record(
                        {
                            "event": "repo_process_guard_drained",
                            "violation": process_sentinel.violation_payload(violation),
                        }
                    )
                    process_sentinel.terminate_group(
                        group.pgid,
                        grace=self._drain_grace_sec,
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
    ) -> RepoProcessMemorySentinel | None:
        if not self.limits.enabled or _sentinel_active():
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
    ) as sentinel:
        yield HarnessGuardScope(limits=resolved_limits, sentinel=sentinel)
