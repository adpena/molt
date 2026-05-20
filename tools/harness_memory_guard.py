#!/usr/bin/env python3
from __future__ import annotations

import contextlib
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import signal
import subprocess
import threading
import time
from collections.abc import Callable, Mapping, Sequence

try:
    from tools import memory_guard, process_sentinel
except ModuleNotFoundError:  # pragma: no cover - direct script import from tools/
    import memory_guard  # type: ignore
    import process_sentinel  # type: ignore


DEFAULT_POLL_INTERVAL_SEC = 0.10
TRUE_VALUES = {"1", "true", "yes", "on"}
FALSE_VALUES = {"0", "false", "no", "off"}


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


@dataclass(frozen=True, slots=True)
class HarnessMemoryLimits:
    enabled: bool
    max_process_rss_gb: float
    max_total_rss_gb: float
    max_global_rss_gb: float
    poll_interval: float
    max_process_rss_kb: int = field(init=False, repr=False)
    max_total_rss_kb: int = field(init=False, repr=False)
    max_global_rss_kb: int = field(init=False, repr=False)

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


def _normalize_prefix(prefix: str) -> str:
    return prefix.strip().upper().rstrip("_")


def _effective_env(env: Mapping[str, str] | None) -> Mapping[str, str]:
    if env is None:
        return os.environ
    merged = dict(os.environ)
    merged.update(env)
    return merged


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
    for name in names:
        raw = env.get(name)
        if raw is None or not raw.strip():
            continue
        try:
            return float(raw)
        except ValueError:
            continue
    return default


def limits_from_env(
    prefix: str,
    env: Mapping[str, str] | None = None,
) -> HarnessMemoryLimits:
    source = _effective_env(env)
    normalized = _normalize_prefix(prefix)
    enabled = _env_bool(
        source,
        [f"{normalized}_MEMORY_GUARD", "MOLT_MEMORY_GUARD"],
        default=True,
    )
    process_gb = _env_float(
        source,
        [
            f"{normalized}_MAX_PROCESS_RSS_GB",
            f"{normalized}_MAX_RSS_GB",
            "MOLT_MAX_PROCESS_RSS_GB",
            "MOLT_MAX_RSS_GB",
        ],
        default=memory_guard.DEFAULT_MAX_RSS_GB,
    )
    total_gb = _env_float(
        source,
        [
            f"{normalized}_MAX_TOTAL_RSS_GB",
            f"{normalized}_MAX_TREE_RSS_GB",
            "MOLT_MAX_TOTAL_RSS_GB",
            "MOLT_MAX_TREE_RSS_GB",
        ],
        default=memory_guard.DEFAULT_MAX_TOTAL_RSS_GB,
    )
    global_gb = _env_float(
        source,
        [
            f"{normalized}_GLOBAL_RSS_LIMIT_GB",
            f"{normalized}_MAX_GLOBAL_RSS_GB",
            "MOLT_GLOBAL_RSS_LIMIT_GB",
            "MOLT_MAX_GLOBAL_RSS_GB",
        ],
        default=memory_guard.DEFAULT_MAX_GLOBAL_RSS_GB,
    )
    poll_interval = _env_float(
        source,
        [f"{normalized}_MEMORY_GUARD_POLL_SEC", "MOLT_MEMORY_GUARD_POLL_SEC"],
        default=DEFAULT_POLL_INTERVAL_SEC,
    )
    if poll_interval <= 0:
        poll_interval = DEFAULT_POLL_INTERVAL_SEC
    return HarnessMemoryLimits(
        enabled=enabled,
        max_process_rss_gb=process_gb,
        max_total_rss_gb=total_gb,
        max_global_rss_gb=global_gb,
        poll_interval=poll_interval,
    )


def limits_summary(limits: HarnessMemoryLimits) -> dict[str, object]:
    return {
        "enabled": limits.enabled,
        "max_process_rss_gb": limits.max_process_rss_gb,
        "max_total_rss_gb": limits.max_total_rss_gb,
        "max_global_rss_gb": limits.max_global_rss_gb,
        "poll_interval": limits.poll_interval,
    }


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
) -> str:
    if violation is None:
        return ""
    limit_gb = (
        limits.max_total_rss_gb
        if violation.scope == "process_tree"
        else limits.max_process_rss_gb
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
    guarded = memory_guard.run_guarded(
        list(command),
        max_rss_kb=resolved_limits.max_process_rss_kb,
        max_total_rss_kb=resolved_limits.max_total_rss_kb,
        poll_interval=resolved_limits.poll_interval,
        cwd=cwd,
        env=env,
        timeout=timeout,
        capture_output=capture_output,
        child_rlimit_kb=resolved_limits.max_process_rss_kb,
        input=input,
        stream=stream,
    )
    stderr = guarded.stderr or ""
    if guarded.violation is not None:
        stderr += _guard_stderr_message(guarded.violation, resolved_limits)
    elif not guarded.timed_out:
        stderr += _guard_exit_signal_message(guarded.returncode)
    return GuardedCompletedProcess(
        list(command),
        guarded.returncode,
        guarded.stdout,
        stderr,
        elapsed_s=guarded.elapsed_s,
    )


def batch_process_group_kwargs(
    limits: HarnessMemoryLimits | None = None,
) -> dict[str, object]:
    resolved_limits = limits or limits_from_env("MOLT")
    if not resolved_limits.enabled or os.name != "posix":
        return {}
    return {
        "start_new_session": True,
        "preexec_fn": _child_resource_limit_preexec(
            resolved_limits.max_process_rss_kb
        ),
    }


def _child_resource_limit_preexec(limit_kb: int) -> Callable[[], None]:
    def apply_limit() -> None:
        memory_guard._apply_child_resource_limit(limit_kb)

    return apply_limit


def force_close_process_group(proc: subprocess.Popen[str]) -> None:
    if proc.poll() is not None:
        return
    if os.name == "posix":
        with contextlib.suppress(ProcessLookupError, PermissionError, OSError):
            os.killpg(proc.pid, signal.SIGTERM)
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                return
            time.sleep(0.05)
        with contextlib.suppress(ProcessLookupError, PermissionError, OSError):
            os.killpg(proc.pid, signal.SIGKILL)
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
    ) -> None:
        self._repo_root = repo_root
        self._artifact_root = artifact_root
        self._label = label
        self._limits = limits
        self._drain_on_exit = drain_on_exit
        self._drain_grace_sec = max(0.0, drain_grace_sec)
        self._drain_until_clean_sec = max(0.0, drain_until_clean_sec)
        self._drain_max_runtime_sec = max(0.0, drain_max_runtime_sec)
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._baseline_pgids: set[int] = set()
        self.tripped = False
        self.events_path = artifact_root / "memory_guard" / f"{label}_sentinel.jsonl"

    def __enter__(self) -> "RepoProcessMemorySentinel":
        if not self._limits.enabled:
            return self
        self._baseline_pgids = self._current_group_pgids()
        self._thread = threading.Thread(
            target=self._run,
            name=f"{self._label}-memory-sentinel",
            daemon=True,
        )
        self._thread.start()
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=max(0.5, self._limits.poll_interval * 2))
        if self._limits.enabled and self._drain_on_exit:
            self.drain_new_processes()

    def _record(self, payload: dict[str, object]) -> None:
        payload.setdefault("label", self._label)
        payload.setdefault("ts", time.time())
        _append_jsonl(self.events_path, payload)

    def _run(self) -> None:
        while not self._stop.wait(self._limits.poll_interval):
            self.scan_once()

    def _current_groups(self) -> list[process_sentinel.ProcessGroup]:
        return process_sentinel.process_groups(
            memory_guard.sample_processes(),
            root=self._repo_root,
            self_pid=os.getpid(),
            self_pgid=os.getpgrp(),
        )

    def _current_group_pgids(self) -> set[int]:
        try:
            return {group.pgid for group in self._current_groups()}
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
            violations = process_sentinel.find_violations(
                groups,
                max_process_kb=self._limits.max_process_rss_kb,
                max_group_kb=self._limits.max_total_rss_kb,
                max_global_kb=self._limits.max_global_rss_kb,
            )
            if not violations:
                return
            self.tripped = True
            for violation in violations:
                self._record(
                    {
                        "event": "repo_process_guard_tripped",
                        "violation": process_sentinel.violation_payload(violation),
                    }
                )
                process_sentinel.terminate_group(violation.pgid, grace=0.25)
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
                    if group.pgid in drained_pgids:
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
                            "violation": process_sentinel.violation_payload(
                                violation
                            ),
                        }
                    )
                    process_sentinel.terminate_group(
                        group.pgid,
                        grace=self._drain_grace_sec,
                    )
                    drained_pgids.add(group.pgid)
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
    )
