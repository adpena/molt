#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Callable, Mapping, Sequence
import contextlib
from dataclasses import dataclass
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time


# Fallbacks used only when the host cannot report physical/available memory.
DEFAULT_MAX_RSS_GB = 12.0
DEFAULT_MAX_TOTAL_RSS_GB = 18.0
DEFAULT_MAX_GLOBAL_RSS_GB = 36.0
DEFAULT_HARD_MAX_RSS_GB = 112.0
DEFAULT_HARD_MAX_GLOBAL_RSS_GB = 4096.0
DEFAULT_POLL_INTERVAL_SEC = 0.10
DEFAULT_SAMPLES_MAX_MB = 2.0
DEFAULT_MEMORY_RESERVE_FRACTION = 0.06
DEFAULT_MEMORY_RESERVE_MIN_GB = 4.0
DEFAULT_MEMORY_RESERVE_MAX_GB = 12.0
DEFAULT_GLOBAL_FRACTION_OF_USABLE = 0.97
DEFAULT_TOTAL_FRACTION_OF_GLOBAL = 0.60
DEFAULT_PROCESS_FRACTION_OF_TOTAL = 0.75
GUARD_RETURN_CODE = 137
TIMEOUT_RETURN_CODE = 124
INTERNAL_COMMAND_ENV = "MOLT_MEMORY_GUARD_COMMAND_JSON"
INTERNAL_WORKER_ENV = "MOLT_MEMORY_GUARD_INTERNAL"
INTERNAL_CHILD_RUNNER_ENV = "MOLT_MEMORY_GUARD_CHILD_RUNNER"
INTERNAL_CHILD_COMMAND_ENV = "MOLT_MEMORY_GUARD_CHILD_COMMAND_JSON"
INTERNAL_CHILD_RLIMIT_KB_ENV = "MOLT_MEMORY_GUARD_CHILD_RLIMIT_KB"
INTERNAL_CHILD_STARTED_FD_ENV = "MOLT_MEMORY_GUARD_CHILD_STARTED_FD"
_INTERNAL_ENV_KEYS = (
    INTERNAL_COMMAND_ENV,
    INTERNAL_WORKER_ENV,
    INTERNAL_CHILD_RUNNER_ENV,
    INTERNAL_CHILD_COMMAND_ENV,
    INTERNAL_CHILD_RLIMIT_KB_ENV,
    INTERNAL_CHILD_STARTED_FD_ENV,
)


@dataclass(frozen=True, slots=True)
class ProcessSample:
    pid: int
    ppid: int
    rss_kb: int
    command: str
    pgid: int | None = None


@dataclass(frozen=True, slots=True)
class RssViolation:
    pid: int
    rss_kb: int
    command: str
    scope: str = "process"

    @property
    def rss_gb(self) -> float:
        return self.rss_kb / (1024 * 1024)


@dataclass(frozen=True, slots=True)
class GuardResult:
    returncode: int
    violation: RssViolation | None
    peak: RssViolation | None
    peak_total: RssViolation | None
    stdout: str
    stderr: str
    timed_out: bool = False
    elapsed_s: float | None = None


@dataclass(frozen=True, slots=True)
class GuardedLaunch:
    command: list[str]
    env: Mapping[str, str] | None
    pass_fds: tuple[int, ...] = ()
    close_fds: tuple[int, ...] = ()
    started_read_fd: int | None = None


@dataclass(frozen=True, slots=True)
class AdaptiveMemoryBudget:
    max_process_rss_gb: float
    max_total_rss_gb: float
    max_global_rss_gb: float
    reserve_gb: float
    physical_gb: float | None
    available_gb: float | None
    source: str


def _normalize_env_prefix(prefix: str | None) -> str:
    if not prefix:
        return ""
    return prefix.strip().upper().rstrip("_")


def _prefixed_names(prefix: str | None, suffixes: Sequence[str]) -> list[str]:
    normalized = _normalize_env_prefix(prefix)
    names: list[str] = []
    if normalized:
        names.extend(f"{normalized}_{suffix}" for suffix in suffixes)
    names.extend(f"MOLT_{suffix}" for suffix in suffixes)
    return list(dict.fromkeys(names))


def _float_env(environ: Mapping[str, str], names: Sequence[str]) -> float | None:
    for name in names:
        raw = environ.get(name)
        if raw is None or not raw.strip():
            continue
        try:
            value = float(raw)
        except ValueError:
            continue
        if value > 0:
            return value
    return None


def _gb_from_bytes(value: int | None) -> float | None:
    if value is None or value <= 0:
        return None
    return value / (1024 * 1024 * 1024)


def _linux_meminfo_bytes(key: str) -> int | None:
    try:
        text = Path("/proc/meminfo").read_text(encoding="utf-8")
    except OSError:
        return None
    for line in text.splitlines():
        if not line.startswith(f"{key}:"):
            continue
        parts = line.split()
        if len(parts) >= 2 and parts[1].isdigit():
            return int(parts[1]) * 1024
    return None


def _darwin_physical_memory_bytes() -> int | None:
    try:
        result = subprocess.run(
            ["sysctl", "-n", "hw.memsize"],
            capture_output=True,
            text=True,
            timeout=1.0,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        result = None
    if result is not None and result.returncode == 0:
        raw = result.stdout.strip()
        if raw.isdigit():
            return int(raw)
    try:
        return int(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES"))
    except (OSError, ValueError):
        return None


def _darwin_available_memory_bytes() -> int | None:
    try:
        result = subprocess.run(
            ["vm_stat"],
            capture_output=True,
            text=True,
            timeout=1.0,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    page_size = None
    pages: dict[str, int] = {}
    for line in result.stdout.splitlines():
        if "page size of" in line:
            for token in line.replace(".", "").split():
                if token.isdigit():
                    page_size = int(token)
                    break
            continue
        if ":" not in line:
            continue
        key, raw_value = line.split(":", 1)
        digits = "".join(ch for ch in raw_value if ch.isdigit())
        if digits:
            pages[key.strip()] = int(digits)
    if page_size is None:
        try:
            page_size = int(os.sysconf("SC_PAGE_SIZE"))
        except (OSError, ValueError):
            return None
    available_pages = sum(
        pages.get(name, 0)
        for name in (
            "Pages free",
            "Pages inactive",
            "Pages speculative",
            "Pages purgeable",
        )
    )
    if available_pages <= 0:
        return None
    return available_pages * page_size


def physical_memory_bytes(
    prefix: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> int | None:
    source = os.environ if environ is None else environ
    override = _float_env(
        source,
        _prefixed_names(prefix, ("TOTAL_MEMORY_GB", "MEMORY_TOTAL_GB")),
    )
    if override is not None:
        return int(override * 1024 * 1024 * 1024)
    if sys.platform.startswith("linux"):
        return _linux_meminfo_bytes("MemTotal")
    if sys.platform == "darwin":
        return _darwin_physical_memory_bytes()
    try:
        return int(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES"))
    except (OSError, ValueError, AttributeError):
        return None


def available_memory_bytes(
    prefix: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> int | None:
    source = os.environ if environ is None else environ
    override = _float_env(
        source,
        _prefixed_names(prefix, ("MEM_AVAILABLE_GB", "MEMORY_AVAILABLE_GB")),
    )
    if override is not None:
        return int(override * 1024 * 1024 * 1024)
    if sys.platform.startswith("linux"):
        return _linux_meminfo_bytes("MemAvailable")
    if sys.platform == "darwin":
        return _darwin_available_memory_bytes()
    return None


def adaptive_memory_budget(
    prefix: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> AdaptiveMemoryBudget:
    source = os.environ if environ is None else environ
    physical_gb = _gb_from_bytes(physical_memory_bytes(prefix, source))
    available_gb = _gb_from_bytes(available_memory_bytes(prefix, source))
    if physical_gb is not None and available_gb is not None:
        available_gb = min(available_gb, physical_gb)
    reserve_override = _float_env(
        source,
        _prefixed_names(prefix, ("MEMORY_RESERVE_GB", "MEM_RESERVE_GB")),
    )
    if reserve_override is not None:
        reserve_gb = reserve_override
    elif physical_gb is not None:
        reserve_gb = min(
            DEFAULT_MEMORY_RESERVE_MAX_GB,
            max(
                DEFAULT_MEMORY_RESERVE_MIN_GB,
                physical_gb * DEFAULT_MEMORY_RESERVE_FRACTION,
            ),
        )
    else:
        reserve_gb = DEFAULT_MEMORY_RESERVE_MIN_GB

    if available_gb is not None:
        usable_gb = available_gb - reserve_gb
        if usable_gb <= 0:
            usable_gb = max(0.25, available_gb * 0.50)
        source_name = "available"
    elif physical_gb is not None:
        usable_gb = physical_gb * 0.75
        source_name = "physical"
    else:
        return AdaptiveMemoryBudget(
            max_process_rss_gb=DEFAULT_MAX_RSS_GB,
            max_total_rss_gb=DEFAULT_MAX_TOTAL_RSS_GB,
            max_global_rss_gb=DEFAULT_MAX_GLOBAL_RSS_GB,
            reserve_gb=reserve_gb,
            physical_gb=None,
            available_gb=None,
            source="fallback",
        )

    global_gb = max(0.25, usable_gb * DEFAULT_GLOBAL_FRACTION_OF_USABLE)
    if physical_gb is not None:
        global_gb = min(global_gb, max(0.25, physical_gb - reserve_gb))
    total_gb = min(
        global_gb,
        max(0.25, global_gb * DEFAULT_TOTAL_FRACTION_OF_GLOBAL),
    )
    process_gb = min(
        total_gb,
        max(0.25, total_gb * DEFAULT_PROCESS_FRACTION_OF_TOTAL),
    )
    return AdaptiveMemoryBudget(
        max_process_rss_gb=process_gb,
        max_total_rss_gb=total_gb,
        max_global_rss_gb=global_gb,
        reserve_gb=reserve_gb,
        physical_gb=physical_gb,
        available_gb=available_gb,
        source=source_name,
    )


def parse_process_table(text: str) -> dict[int, ProcessSample]:
    samples: dict[int, ProcessSample] = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        pid: int
        ppid: int
        rss_kb: int
        command: str
        pgid: int | None
        parts = line.split(None, 4)
        if len(parts) >= 5:
            try:
                pid = int(parts[0])
                ppid = int(parts[1])
                pgid = int(parts[2])
                rss_kb = int(parts[3])
                command = parts[4]
            except ValueError:
                legacy_parts = line.split(None, 3)
                if len(legacy_parts) < 4:
                    continue
                try:
                    pid = int(legacy_parts[0])
                    ppid = int(legacy_parts[1])
                    rss_kb = int(legacy_parts[2])
                except ValueError:
                    continue
                command = legacy_parts[3]
                pgid = None
        else:
            legacy_parts = line.split(None, 3)
            if len(legacy_parts) < 4:
                continue
            try:
                pid = int(legacy_parts[0])
                ppid = int(legacy_parts[1])
                rss_kb = int(legacy_parts[2])
            except ValueError:
                continue
            command = legacy_parts[3]
            pgid = None
        samples[pid] = ProcessSample(
            pid=pid,
            ppid=ppid,
            rss_kb=rss_kb,
            command=command,
            pgid=pgid,
        )
    return samples


def sample_processes() -> dict[int, ProcessSample]:
    result = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,pgid=,rss=,command="],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return {}
    return parse_process_table(result.stdout)


def descendant_pids(samples: Mapping[int, ProcessSample], root_pid: int) -> set[int]:
    descendants = {root_pid}
    changed = True
    while changed:
        changed = False
        for sample in samples.values():
            if sample.pid in descendants:
                continue
            if sample.ppid in descendants:
                descendants.add(sample.pid)
                changed = True
    return descendants


def watched_pids(samples: Mapping[int, ProcessSample], root_pid: int) -> set[int]:
    watched = descendant_pids(samples, root_pid)
    for sample in samples.values():
        if sample.pgid == root_pid:
            watched.add(sample.pid)
    return watched


def peak_rss(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
) -> RssViolation | None:
    watched = watched_pids(samples, root_pid)
    candidates = [sample for pid, sample in samples.items() if pid in watched]
    if not candidates:
        return None
    worst = max(candidates, key=lambda sample: sample.rss_kb)
    return RssViolation(
        pid=worst.pid,
        rss_kb=worst.rss_kb,
        command=worst.command,
    )


def total_rss(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
) -> RssViolation | None:
    watched = watched_pids(samples, root_pid)
    candidates = [sample for pid, sample in samples.items() if pid in watched]
    if not candidates:
        return None
    return RssViolation(
        pid=root_pid,
        rss_kb=sum(sample.rss_kb for sample in candidates),
        command="process tree aggregate",
        scope="process_tree",
    )


def find_rss_violation(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
    max_rss_kb: int,
    max_total_rss_kb: int | None = None,
) -> RssViolation | None:
    watched = watched_pids(samples, root_pid)
    candidates = [
        sample
        for pid, sample in samples.items()
        if pid in watched and sample.rss_kb > max_rss_kb
    ]
    if not candidates:
        if max_total_rss_kb is None:
            return None
        aggregate = total_rss(samples, root_pid=root_pid)
        if aggregate is not None and aggregate.rss_kb > max_total_rss_kb:
            return aggregate
        return None
    worst = max(candidates, key=lambda sample: sample.rss_kb)
    return RssViolation(
        pid=worst.pid,
        rss_kb=worst.rss_kb,
        command=worst.command,
    )


def max_rss_kb_from_gb(value: float) -> int:
    if value <= 0:
        raise ValueError("max RSS must be greater than 0 GB")
    if value >= DEFAULT_HARD_MAX_RSS_GB:
        raise ValueError(f"max RSS must stay below {DEFAULT_HARD_MAX_RSS_GB:g} GB")
    return int(value * 1024 * 1024)


def max_global_rss_kb_from_gb(value: float) -> int:
    if value <= 0:
        raise ValueError("global RSS must be greater than 0 GB")
    if value >= DEFAULT_HARD_MAX_GLOBAL_RSS_GB:
        raise ValueError(
            f"global RSS must stay below {DEFAULT_HARD_MAX_GLOBAL_RSS_GB:g} GB"
        )
    return int(value * 1024 * 1024)


def _samples_max_bytes_from_mb(value: float | None) -> int | None:
    if value is None:
        value = DEFAULT_SAMPLES_MAX_MB
    if value <= 0:
        return None
    return max(1024, int(value * 1024 * 1024))


def _rotate_jsonl_if_needed(
    path: Path, incoming_bytes: int, max_bytes: int | None
) -> None:
    if max_bytes is None:
        return
    try:
        current_size = path.stat().st_size
    except FileNotFoundError:
        return
    except OSError:
        return
    if current_size + incoming_bytes <= max_bytes:
        return
    rotated = path.with_name(f"{path.name}.1")
    with contextlib.suppress(OSError):
        rotated.unlink()
    with contextlib.suppress(OSError):
        path.replace(rotated)


def _append_sample_jsonl(
    path: str,
    *,
    root_pid: int,
    peak: RssViolation | None,
    total: RssViolation | None,
    violation: RssViolation | None,
    max_bytes: int | None = None,
) -> None:
    sample_path = Path(path)
    if sample_path.parent:
        sample_path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "ts": time.time(),
        "root_pid": root_pid,
        "peak": _rss_record_payload(peak),
        "total": _rss_record_payload(total),
        "violation": _rss_record_payload(violation),
    }
    line = json.dumps(payload, sort_keys=True) + "\n"
    _rotate_jsonl_if_needed(sample_path, len(line.encode("utf-8")), max_bytes)
    with sample_path.open("a", encoding="utf-8") as handle:
        handle.write(line)


def _record_gb(record: object) -> str:
    if not isinstance(record, dict):
        return "-"
    value = record.get("rss_gb")
    if isinstance(value, (int, float)):
        return f"{value:.2f}GB"
    return "-"


def _format_sample_payload(payload: dict[str, object]) -> str:
    violation = payload.get("violation")
    if violation is not None:
        return f"memory_guard sample: TRIP violation={_record_gb(violation)}"
    return (
        "memory_guard sample: "
        f"peak={_record_gb(payload.get('peak'))} "
        f"total={_record_gb(payload.get('total'))}"
    )


def _stream_sample_payload(payload: dict[str, object], stream: str) -> None:
    if not stream:
        return
    target = sys.stdout if "stdout" in stream else sys.stderr
    try:
        if "json" in stream:
            print(json.dumps(payload, sort_keys=True), file=target, flush=True)
        else:
            print(_format_sample_payload(payload), file=target, flush=True)
    except Exception:
        return


def _record_sample(
    *,
    root_pid: int,
    peak: RssViolation | None,
    total: RssViolation | None,
    violation: RssViolation | None,
    samples_jsonl: str | None,
    samples_jsonl_max_bytes: int | None,
    stream: str,
) -> None:
    payload = {
        "ts": time.time(),
        "root_pid": root_pid,
        "peak": _rss_record_payload(peak),
        "total": _rss_record_payload(total),
        "violation": _rss_record_payload(violation),
    }
    if samples_jsonl is not None:
        sample_path = Path(samples_jsonl)
        if sample_path.parent:
            sample_path.parent.mkdir(parents=True, exist_ok=True)
        line = json.dumps(payload, sort_keys=True) + "\n"
        _rotate_jsonl_if_needed(
            sample_path,
            len(line.encode("utf-8")),
            samples_jsonl_max_bytes,
        )
        with sample_path.open("a", encoding="utf-8") as handle:
            handle.write(line)
    _stream_sample_payload(payload, stream)


def _terminate_process_group(pid: int) -> None:
    try:
        os.killpg(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    except OSError:
        with contextlib.suppress(ProcessLookupError):
            os.kill(pid, signal.SIGTERM)
        return
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        try:
            os.killpg(pid, 0)
        except ProcessLookupError:
            return
        except OSError:
            return
        time.sleep(0.05)
    try:
        os.killpg(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def _load_json_string_list(environ: Mapping[str, str], name: str) -> list[str]:
    raw = environ.get(name)
    if not raw:
        raise ValueError(f"{name} is required")
    try:
        decoded = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ValueError(f"{name} is invalid JSON") from exc
    if not isinstance(decoded, list) or not all(
        isinstance(item, str) for item in decoded
    ):
        raise ValueError(f"{name} must be a JSON string list")
    if not decoded:
        raise ValueError(f"{name} command must not be empty")
    return decoded


def _apply_child_resource_limit(limit_kb: int) -> None:
    if limit_kb <= 0:
        return
    try:
        import resource  # type: ignore
    except Exception:
        return
    limit_bytes = int(limit_kb * 1024)
    for name in ("RLIMIT_AS", "RLIMIT_DATA", "RLIMIT_RSS"):
        res = getattr(resource, name, None)
        if res is None:
            continue
        try:
            soft, hard = resource.getrlimit(res)
            bounded_hard = (
                limit_bytes
                if hard == resource.RLIM_INFINITY
                else min(int(hard), limit_bytes)
            )
            bounded_soft = (
                limit_bytes
                if soft == resource.RLIM_INFINITY
                else min(int(soft), limit_bytes)
            )
            resource.setrlimit(res, (min(bounded_soft, bounded_hard), bounded_hard))
        except Exception:
            continue


def _run_child_runner(environ: Mapping[str, str]) -> int:
    try:
        command = _load_json_string_list(environ, INTERNAL_CHILD_COMMAND_ENV)
        raw_limit = environ.get(INTERNAL_CHILD_RLIMIT_KB_ENV, "0")
        try:
            limit_kb = int(raw_limit)
        except ValueError as exc:
            raise ValueError(f"{INTERNAL_CHILD_RLIMIT_KB_ENV} must be an int") from exc
    except ValueError as exc:
        print(f"memory_guard child_runner: {exc}", file=sys.stderr)
        return 2
    _apply_child_resource_limit(limit_kb)
    child_env = _child_env_without_internal_keys(environ)
    _write_child_started_timestamp(environ)
    try:
        os.execvpe(command[0], command, child_env)
    except OSError as exc:
        print(f"memory_guard child_runner: exec failed: {exc}", file=sys.stderr)
        return 127
    return 127


def _write_child_started_timestamp(environ: Mapping[str, str]) -> None:
    raw_fd = environ.get(INTERNAL_CHILD_STARTED_FD_ENV)
    if not raw_fd:
        return
    try:
        fd = int(raw_fd)
    except ValueError:
        return
    try:
        os.write(fd, f"{time.monotonic_ns()}\n".encode("ascii"))
    except OSError:
        pass
    with contextlib.suppress(OSError):
        os.close(fd)


def _child_runner_env(
    environ: Mapping[str, str],
    command: Sequence[str],
    *,
    child_rlimit_kb: int,
    child_started_fd: int | None = None,
) -> dict[str, str]:
    runner_env = dict(environ)
    runner_env[INTERNAL_CHILD_RUNNER_ENV] = "1"
    runner_env[INTERNAL_CHILD_COMMAND_ENV] = json.dumps(list(command))
    runner_env[INTERNAL_CHILD_RLIMIT_KB_ENV] = str(child_rlimit_kb)
    if child_started_fd is not None:
        runner_env[INTERNAL_CHILD_STARTED_FD_ENV] = str(child_started_fd)
    return runner_env


def _guarded_launch(
    command: Sequence[str],
    env: Mapping[str, str] | None,
    *,
    child_rlimit_kb: int | None,
) -> GuardedLaunch:
    if child_rlimit_kb is None or child_rlimit_kb <= 0:
        return GuardedLaunch(command=list(command), env=env)
    base_env = os.environ if env is None else env
    started_read_fd: int | None = None
    started_write_fd: int | None = None
    pass_fds: tuple[int, ...] = ()
    close_fds: tuple[int, ...] = ()
    if os.name == "posix":
        started_read_fd, started_write_fd = os.pipe()
        pass_fds = (started_write_fd,)
        close_fds = (started_write_fd,)
    return GuardedLaunch(
        command=[sys.executable, str(Path(__file__).resolve())],
        env=_child_runner_env(
            base_env,
            command,
            child_rlimit_kb=child_rlimit_kb,
            child_started_fd=started_write_fd,
        ),
        pass_fds=pass_fds,
        close_fds=close_fds,
        started_read_fd=started_read_fd,
    )


def _close_fds(fds: Sequence[int | None]) -> None:
    for fd in fds:
        if fd is None:
            continue
        with contextlib.suppress(OSError):
            os.close(fd)


def _read_child_started_at(fd: int | None) -> float | None:
    if fd is None:
        return None
    try:
        raw = os.read(fd, 64)
    except OSError:
        return None
    finally:
        with contextlib.suppress(OSError):
            os.close(fd)
    try:
        return int(raw.strip()) / 1_000_000_000
    except ValueError:
        return None


def run_guarded(
    command: Sequence[str],
    *,
    max_rss_kb: int,
    max_total_rss_kb: int | None = None,
    poll_interval: float,
    sampler: Callable[[], Mapping[int, ProcessSample]] = sample_processes,
    capture_output: bool = True,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    samples_jsonl: str | None = None,
    samples_jsonl_max_bytes: int | None = None,
    stream: str = "",
    child_rlimit_kb: int | None = None,
    input: str | None = None,
) -> GuardResult:
    if not command:
        raise ValueError("command is required")
    if poll_interval <= 0:
        raise ValueError("poll interval must be greater than 0")
    if timeout is not None and timeout <= 0:
        raise ValueError("timeout must be greater than 0")
    start = time.monotonic()
    launch = _guarded_launch(
        command,
        dict(env) if env is not None else None,
        child_rlimit_kb=child_rlimit_kb,
    )
    popen_kwargs: dict[str, object] = {
        "cwd": cwd,
        "env": dict(launch.env) if launch.env is not None else None,
        "stdout": subprocess.PIPE if capture_output else None,
        "stderr": subprocess.PIPE if capture_output else None,
        "stdin": subprocess.PIPE if input is not None else None,
        "text": True,
        "start_new_session": True,
    }
    if launch.pass_fds:
        popen_kwargs["pass_fds"] = launch.pass_fds
    try:
        proc = subprocess.Popen(launch.command, **popen_kwargs)
    except Exception:
        _close_fds((*launch.close_fds, launch.started_read_fd))
        raise
    _close_fds(launch.close_fds)
    stdin_thread: threading.Thread | None = None
    if input is not None and proc.stdin is not None:
        stdin_handle = proc.stdin
        proc.stdin = None

        def _feed_stdin() -> None:
            try:
                stdin_handle.write(input)
                stdin_handle.close()
            except (BrokenPipeError, OSError, ValueError):
                with contextlib.suppress(OSError, ValueError):
                    stdin_handle.close()

        stdin_thread = threading.Thread(
            target=_feed_stdin,
            name="memory-guard-stdin-feeder",
            daemon=True,
        )
        stdin_thread.start()
    violation: RssViolation | None = None
    peak: RssViolation | None = None
    peak_total: RssViolation | None = None
    timed_out = False
    while True:
        if timeout is not None and time.monotonic() - start >= timeout:
            timed_out = True
            _terminate_process_group(proc.pid)
            break
        samples = sampler()
        observed_peak = peak_rss(samples, root_pid=proc.pid)
        if observed_peak is not None and (
            peak is None or observed_peak.rss_kb > peak.rss_kb
        ):
            peak = observed_peak
        observed_total = total_rss(samples, root_pid=proc.pid)
        if observed_total is not None and (
            peak_total is None or observed_total.rss_kb > peak_total.rss_kb
        ):
            peak_total = observed_total
        violation = find_rss_violation(
            samples,
            root_pid=proc.pid,
            max_rss_kb=max_rss_kb,
            max_total_rss_kb=max_total_rss_kb,
        )
        if violation is not None:
            _record_sample(
                root_pid=proc.pid,
                peak=observed_peak,
                total=observed_total,
                violation=violation,
                samples_jsonl=samples_jsonl,
                samples_jsonl_max_bytes=samples_jsonl_max_bytes,
                stream=stream,
            )
            _terminate_process_group(proc.pid)
            break
        if samples_jsonl is not None or stream:
            _record_sample(
                root_pid=proc.pid,
                peak=observed_peak,
                total=observed_total,
                violation=None,
                samples_jsonl=samples_jsonl,
                samples_jsonl_max_bytes=samples_jsonl_max_bytes,
                stream=stream,
            )
        if proc.poll() is not None:
            break
        wait_timeout = poll_interval
        if timeout is not None:
            remaining = timeout - (time.monotonic() - start)
            wait_timeout = max(0.0, min(wait_timeout, remaining))
        try:
            proc.wait(timeout=wait_timeout)
            break
        except subprocess.TimeoutExpired:
            pass
    finished = time.monotonic()
    stdout, stderr = proc.communicate()
    if stdin_thread is not None:
        stdin_thread.join(timeout=1.0)
    child_started = _read_child_started_at(launch.started_read_fd)
    elapsed_start = child_started if child_started is not None else start
    elapsed_s = max(0.0, finished - elapsed_start)
    returncode = proc.returncode
    if violation is not None:
        returncode = GUARD_RETURN_CODE
    if timed_out:
        returncode = TIMEOUT_RETURN_CODE
        timeout_msg = f"memory_guard: timeout after {timeout:.2f}s\n"
        stderr = f"{stderr or ''}{timeout_msg}"
    return GuardResult(
        returncode=returncode,
        violation=violation,
        peak=peak,
        peak_total=peak_total,
        stdout=stdout or "",
        stderr=stderr or "",
        timed_out=timed_out,
        elapsed_s=elapsed_s,
    )


def _rss_record_payload(record: RssViolation | None) -> dict[str, object] | None:
    if record is None:
        return None
    return {
        "pid": record.pid,
        "rss_kb": record.rss_kb,
        "rss_gb": record.rss_gb,
        "command": record.command,
        "scope": record.scope,
    }


def exit_signal_payload(returncode: int) -> dict[str, object] | None:
    conventional_shell_status = False
    if returncode < 0:
        signo = -returncode
    elif 129 <= returncode <= 192:
        signo = returncode - 128
        conventional_shell_status = True
    else:
        return None
    with contextlib.suppress(ValueError):
        signame = signal.Signals(signo).name
        return {
            "signal": signo,
            "name": signame,
            "conventional_shell_status": conventional_shell_status,
        }
    return {
        "signal": signo,
        "name": None,
        "conventional_shell_status": conventional_shell_status,
    }


_exit_signal_payload = exit_signal_payload


def _write_summary_json(
    path: str,
    *,
    command: Sequence[str],
    max_rss_kb: int,
    max_total_rss_kb: int | None,
    child_rlimit_kb: int | None,
    result: GuardResult,
) -> None:
    summary_path = Path(path)
    if summary_path.parent:
        summary_path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "command": list(command),
        "returncode": result.returncode,
        "elapsed_s": result.elapsed_s,
        "max_rss_kb": max_rss_kb,
        "max_rss_gb": max_rss_kb / (1024 * 1024),
        "max_total_rss_kb": max_total_rss_kb,
        "max_total_rss_gb": (
            None if max_total_rss_kb is None else max_total_rss_kb / (1024 * 1024)
        ),
        "child_rlimit_kb": child_rlimit_kb,
        "child_rlimit_gb": (
            None if child_rlimit_kb is None else child_rlimit_kb / (1024 * 1024)
        ),
        "violation": _rss_record_payload(result.violation),
        "peak": _rss_record_payload(result.peak),
        "peak_total": _rss_record_payload(result.peak_total),
        "timed_out": result.timed_out,
        "exit_signal": (
            None
            if result.violation is not None or result.timed_out
            else _exit_signal_payload(result.returncode)
        ),
    }
    summary_path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run a command with a process-tree/process-group RSS ceiling."
    )
    parser.add_argument(
        "--max-rss-gb",
        type=float,
        default=None,
        help=(
            "Abort if any child process exceeds this RSS; must be "
            f"<{DEFAULT_HARD_MAX_RSS_GB:g}GB "
            "(default: adaptive from live available memory)."
        ),
    )
    parser.add_argument(
        "--max-total-rss-gb",
        type=float,
        default=None,
        help=(
            "Abort if the watched process tree exceeds this aggregate RSS; "
            f"must be <{DEFAULT_HARD_MAX_RSS_GB:g}GB "
            "(default: adaptive from live available memory)."
        ),
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=DEFAULT_POLL_INTERVAL_SEC,
        help=(
            "Process sampling interval in seconds "
            f"(default: {DEFAULT_POLL_INTERVAL_SEC})."
        ),
    )
    parser.add_argument(
        "--summary-json",
        help="Write command result, violation, and peak RSS details as JSON.",
    )
    parser.add_argument(
        "--samples-jsonl",
        help="Append per-poll peak and process-tree RSS samples as JSONL.",
    )
    parser.add_argument(
        "--samples-max-mb",
        type=float,
        default=DEFAULT_SAMPLES_MAX_MB,
        help=(
            "Rotate --samples-jsonl after this many MB; set <=0 to disable "
            f"rotation (default: {DEFAULT_SAMPLES_MAX_MB})."
        ),
    )
    parser.add_argument(
        "--stream",
        choices=("stderr", "stdout", "json-stderr", "json-stdout"),
        default="",
        help="Emit per-poll guard samples to this stream without writing artifacts.",
    )
    parser.add_argument(
        "--child-rlimit-gb",
        type=float,
        default=None,
        help=(
            "Apply an OS resource limit to the direct guarded child before exec; "
            "defaults to --max-rss-gb. Set <=0 to disable this layer."
        ),
    )
    parser.add_argument(
        "--timeout",
        type=float,
        help="Abort the command if wall-clock runtime exceeds this many seconds.",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    return parser


def _load_internal_command(environ: Mapping[str, str]) -> list[str] | None:
    if environ.get(INTERNAL_WORKER_ENV) != "1":
        return None
    return _load_json_string_list(environ, INTERNAL_COMMAND_ENV)


def _child_env_without_internal_keys(environ: Mapping[str, str]) -> dict[str, str]:
    child_env = dict(environ)
    for key in _INTERNAL_ENV_KEYS:
        child_env.pop(key, None)
    return child_env


def _worker_env(environ: Mapping[str, str], command: Sequence[str]) -> dict[str, str]:
    worker_env = dict(environ)
    worker_env[INTERNAL_COMMAND_ENV] = json.dumps(list(command))
    worker_env[INTERNAL_WORKER_ENV] = "1"
    return worker_env


def _worker_argv(args: argparse.Namespace) -> list[str]:
    worker_args = [
        sys.executable,
        str(Path(__file__).resolve()),
        "--poll-interval",
        str(args.poll_interval),
    ]
    if args.max_rss_gb is not None:
        worker_args.extend(["--max-rss-gb", str(args.max_rss_gb)])
    if args.max_total_rss_gb is not None:
        worker_args.extend(["--max-total-rss-gb", str(args.max_total_rss_gb)])
    if args.summary_json:
        worker_args.extend(["--summary-json", args.summary_json])
    if args.samples_jsonl:
        worker_args.extend(["--samples-jsonl", args.samples_jsonl])
        worker_args.extend(["--samples-max-mb", str(args.samples_max_mb)])
    if args.stream:
        worker_args.extend(["--stream", args.stream])
    if args.child_rlimit_gb is not None:
        worker_args.extend(["--child-rlimit-gb", str(args.child_rlimit_gb)])
    if args.timeout is not None:
        worker_args.extend(["--timeout", str(args.timeout)])
    return worker_args


def main(
    argv: Sequence[str] | None = None,
    *,
    hide_command_argv: bool = False,
    execve: Callable[[str, Sequence[str], Mapping[str, str]], object] = os.execve,
    environ: Mapping[str, str] | None = None,
) -> int:
    current_env = os.environ if environ is None else environ
    if current_env.get(INTERNAL_CHILD_RUNNER_ENV) == "1":
        return _run_child_runner(current_env)
    args = _parser().parse_args(argv)
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        try:
            internal_command = _load_internal_command(current_env)
        except ValueError as exc:
            print(f"memory_guard: {exc}", file=sys.stderr)
            return 2
        if internal_command is None:
            print("memory_guard: command is required", file=sys.stderr)
            return 2
        command = internal_command
    try:
        budget = adaptive_memory_budget(environ=current_env)
        max_rss_gb = (
            budget.max_process_rss_gb
            if args.max_rss_gb is None
            else float(args.max_rss_gb)
        )
        max_total_rss_gb = (
            budget.max_total_rss_gb
            if args.max_total_rss_gb is None
            else float(args.max_total_rss_gb)
        )
        max_rss_kb = max_rss_kb_from_gb(max_rss_gb)
        max_total_rss_kb = max_rss_kb_from_gb(max_total_rss_gb)
        poll_interval = float(args.poll_interval)
        if poll_interval <= 0:
            raise ValueError("poll interval must be greater than 0")
        if args.timeout is not None and args.timeout <= 0:
            raise ValueError("timeout must be greater than 0")
        samples_jsonl_max_bytes = _samples_max_bytes_from_mb(args.samples_max_mb)
        child_rlimit_gb = (
            max_rss_gb if args.child_rlimit_gb is None else float(args.child_rlimit_gb)
        )
        child_rlimit_kb = (
            None if child_rlimit_gb <= 0 else max_rss_kb_from_gb(child_rlimit_gb)
        )
    except ValueError as exc:
        print(f"memory_guard: {exc}", file=sys.stderr)
        return 2
    if hide_command_argv and current_env.get(INTERNAL_WORKER_ENV) != "1":
        worker_argv = _worker_argv(args)
        execve(
            sys.executable,
            worker_argv,
            _worker_env(current_env, command),
        )
        print("memory_guard: failed to exec internal worker", file=sys.stderr)
        return 2
    result = run_guarded(
        command,
        max_rss_kb=max_rss_kb,
        max_total_rss_kb=max_total_rss_kb,
        poll_interval=poll_interval,
        capture_output=False,
        timeout=args.timeout,
        env=_child_env_without_internal_keys(current_env),
        samples_jsonl=args.samples_jsonl,
        samples_jsonl_max_bytes=samples_jsonl_max_bytes,
        stream=args.stream,
        child_rlimit_kb=child_rlimit_kb,
    )
    if args.summary_json:
        try:
            _write_summary_json(
                args.summary_json,
                command=command,
                max_rss_kb=max_rss_kb,
                max_total_rss_kb=max_total_rss_kb,
                child_rlimit_kb=child_rlimit_kb,
                result=result,
            )
        except OSError as exc:
            print(f"memory_guard: failed to write summary JSON: {exc}", file=sys.stderr)
            return 2 if result.returncode == 0 else result.returncode
    if result.violation is not None:
        limit_gb = (
            max_total_rss_gb if result.violation.scope == "process_tree" else max_rss_gb
        )
        print(
            "memory_guard: RSS limit exceeded: "
            f"pid={result.violation.pid} "
            f"rss={result.violation.rss_gb:.2f}GB "
            f"limit={limit_gb:.2f}GB "
            f"scope={result.violation.scope} "
            f"command={result.violation.command}",
            file=sys.stderr,
        )
    if result.timed_out:
        print(
            f"memory_guard: timeout after {args.timeout:.2f}s",
            file=sys.stderr,
        )
    exit_signal = _exit_signal_payload(result.returncode)
    if exit_signal is not None and result.violation is None and not result.timed_out:
        signame = exit_signal["name"] or f"signal {exit_signal['signal']}"
        print(
            "memory_guard: command exited with "
            f"{signame} status ({result.returncode}); no RSS violation observed",
            file=sys.stderr,
        )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main(hide_command_argv=True))
