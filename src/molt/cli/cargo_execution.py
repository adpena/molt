from __future__ import annotations

import contextlib
from dataclasses import dataclass
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import sys
import time
from typing import Any, Callable, Iterator, Mapping, Sequence

from molt.cargo_execution_policy import (
    _wrapper_is_sccache,
    cargo_compiler_wrappers,
    normalize_cargo_environment,
    sccache_compiler_wrappers,
    without_sccache_compiler_wrappers,
)
from molt.dx import (
    DEFAULT_SCCACHE_CACHE_SIZE,
    _BYTES_PER_CARGO_JOB,  # noqa: F401 (re-exported for compat)
    _CARGO_JOB_MEMORY_HEADROOM,  # noqa: F401 (re-exported for compat)
    _memory_bounded_cargo_jobs,
    _total_system_memory_bytes,  # noqa: F401 (re-exported for compat)
    development_artifact_env,
    development_artifacts_requested,
)
from molt.cli.build_locks import _release_file_lock, _try_acquire_file_lock
from molt.cli.command_runtime import _run_completed_command
from molt.cli.project_roots import _find_molt_root


_MAX_CONCURRENT_BUILDS = 2
_CARGO_ATTEMPT_TEXT_LIMIT = 128 * 1024
_CARGO_ATTEMPT_EVIDENCE_SCHEMA = "molt.cargo-attempt.v1"
_CARGO_EXECUTION_EVIDENCE_SCHEMA = "molt.cargo-execution.v1"


def _bounded_cargo_attempt_text(text: str) -> str:
    if len(text) <= _CARGO_ATTEMPT_TEXT_LIMIT:
        return text
    half = _CARGO_ATTEMPT_TEXT_LIMIT // 2
    omitted = len(text) - (half * 2)
    return (
        text[:half]
        + f"\n... <{omitted} chars omitted from cargo attempt evidence> ...\n"
        + text[-half:]
    )


def _text_output(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def _rss_bytes(result: subprocess.CompletedProcess[Any], attr: str) -> int | None:
    sample = getattr(result, attr, None)
    rss_kb = getattr(sample, "rss_kb", None)
    return int(rss_kb) * 1024 if isinstance(rss_kb, int) and rss_kb >= 0 else None


def _cargo_result_signal(
    result: subprocess.CompletedProcess[Any], stderr: str
) -> dict[str, object] | None:
    guard_signal = getattr(result, "guard_signal", None)
    if isinstance(guard_signal, int) and guard_signal > 0:
        try:
            name = signal.Signals(guard_signal).name
        except ValueError:
            name = str(guard_signal)
        return {"number": guard_signal, "name": name, "source": "guard"}
    if result.returncode < 0:
        number = -result.returncode
        try:
            name = signal.Signals(number).name
        except ValueError:
            name = str(number)
        return {"number": number, "name": name, "source": "returncode"}
    match = re.search(
        r"\bsignal:\s*(?:(\d+)\s*,\s*)?(SIG[A-Z0-9]+)(?::[^\r\n)]*)?",
        stderr,
        flags=re.IGNORECASE,
    )
    if match is None:
        return None
    number = int(match.group(1)) if match.group(1) is not None else None
    return {
        "number": number,
        "name": match.group(2).upper(),
        "source": "cargo-diagnostic",
    }


@dataclass(frozen=True, slots=True)
class CargoAttemptEvidence:
    index: int
    wrapper: str | None
    returncode: int
    signal: dict[str, object] | None
    timed_out: bool
    duration_seconds: float
    peak_process_rss_bytes: int | None
    peak_tree_rss_bytes: int | None
    failure_kind: str | None
    stdout: str
    stderr: str

    def json_payload(self) -> dict[str, object]:
        return {
            "schema": _CARGO_ATTEMPT_EVIDENCE_SCHEMA,
            "index": self.index,
            "wrapper": self.wrapper,
            "returncode": self.returncode,
            "signal": self.signal,
            "timed_out": self.timed_out,
            "duration_seconds": round(self.duration_seconds, 6),
            "peak_process_rss_bytes": self.peak_process_rss_bytes,
            "peak_tree_rss_bytes": self.peak_tree_rss_bytes,
            "failure_kind": self.failure_kind,
            "stdout": self.stdout,
            "stderr": self.stderr,
        }


class CargoExecutionResult(subprocess.CompletedProcess[str]):
    """Terminal Cargo result plus bounded, ordered evidence for every attempt."""

    def __init__(
        self,
        terminal: subprocess.CompletedProcess[Any],
        *,
        attempts: Sequence[CargoAttemptEvidence],
        retry_reason: str | None,
    ) -> None:
        stdout = _text_output(terminal.stdout)
        stderr = _text_output(terminal.stderr)
        super().__init__(terminal.args, terminal.returncode, stdout, stderr)
        self.attempts = tuple(attempts)
        self.retry_reason = retry_reason
        self.elapsed_s = sum(attempt.duration_seconds for attempt in attempts)
        self.peak_process_rss_bytes = max(
            (
                attempt.peak_process_rss_bytes
                for attempt in attempts
                if attempt.peak_process_rss_bytes is not None
            ),
            default=None,
        )
        self.peak_tree_rss_bytes = max(
            (
                attempt.peak_tree_rss_bytes
                for attempt in attempts
                if attempt.peak_tree_rss_bytes is not None
            ),
            default=None,
        )
        self.timed_out = bool(getattr(terminal, "timed_out", False))
        self.guard_signal = getattr(terminal, "guard_signal", None)


def cargo_execution_evidence(
    result: subprocess.CompletedProcess[Any],
) -> dict[str, object]:
    """Return one stable JSON-ready execution payload for Cargo build failures."""
    attempts = getattr(result, "attempts", None)
    typed_attempts = (
        [attempt for attempt in attempts if isinstance(attempt, CargoAttemptEvidence)]
        if isinstance(attempts, tuple)
        else []
    )
    if (
        isinstance(attempts, tuple)
        and typed_attempts
        and len(typed_attempts) == len(attempts)
    ):
        attempt_records = typed_attempts
    else:
        stderr = _text_output(result.stderr)
        stdout = _text_output(result.stdout)
        elapsed = getattr(result, "elapsed_s", 0.0)
        duration = float(elapsed) if isinstance(elapsed, (int, float)) else 0.0
        attempt_records = [
            CargoAttemptEvidence(
                index=1,
                wrapper=None,
                returncode=result.returncode,
                signal=_cargo_result_signal(result, f"{stderr}\n{stdout}"),
                timed_out=bool(getattr(result, "timed_out", False)),
                duration_seconds=max(0.0, duration),
                peak_process_rss_bytes=_rss_bytes(result, "peak"),
                peak_tree_rss_bytes=_rss_bytes(result, "peak_total"),
                failure_kind=None,
                stdout=_bounded_cargo_attempt_text(stdout),
                stderr=_bounded_cargo_attempt_text(stderr),
            )
        ]
    final = attempt_records[-1]
    attempt_payloads = [attempt.json_payload() for attempt in attempt_records]
    durations = [attempt.duration_seconds for attempt in attempt_records]
    process_peaks = [
        attempt.peak_process_rss_bytes
        for attempt in attempt_records
        if attempt.peak_process_rss_bytes is not None
    ]
    tree_peaks = [
        attempt.peak_tree_rss_bytes
        for attempt in attempt_records
        if attempt.peak_tree_rss_bytes is not None
    ]
    retry_reason = getattr(result, "retry_reason", None)
    return {
        "schema": _CARGO_EXECUTION_EVIDENCE_SCHEMA,
        "attempt_count": len(attempt_payloads),
        "retry_reason": retry_reason if isinstance(retry_reason, str) else None,
        "timed_out": final.timed_out,
        "duration_seconds": round(sum(durations), 6),
        "peak_process_rss_bytes": max(process_peaks, default=None),
        "peak_tree_rss_bytes": max(tree_peaks, default=None),
        "signal": final.signal,
        "attempts": attempt_payloads,
    }


# Cargo `--jobs` memory-bounding authority now lives in molt.dx (the resource
# authority, importable without a cycle). See dx._memory_bounded_cargo_jobs.
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


def _maybe_enable_lld_link(env: dict[str, str]) -> None:
    """On Windows, use LLVM ``lld-link`` as the MSVC linker when it is available.

    ``lld-link`` links the backend daemon / runtime staticlib dramatically faster
    than the serial MSVC ``link.exe`` (the link is the build long pole). This is
    PORTABLE — a no-op where ``lld-link`` is absent (CI / boxes without LLVM keep
    ``link.exe``), and an explicit operator ``CARGO_TARGET_..._LINKER`` always wins.
    It uses the target-specific linker env var (NOT a rustflag, which build paths
    that set ``RUSTFLAGS`` would replace), so it survives every cargo invocation and
    only affects the native ``x86_64-pc-windows-msvc`` target (never wasm).
    """
    if os.name != "nt":
        return
    key = "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER"
    if env.get(key, "").strip():
        return
    lld = shutil.which("lld-link")
    if lld:
        env[key] = lld


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
        result = _run_completed_command(
            [sccache, "--show-stats"],
            cwd=Path.cwd(),
            env=os.environ.copy(),
            capture_output=True,
            memory_guard_prefix="MOLT_BUILD",
            timeout=15,
        )
        return result.returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def _maybe_enable_sccache(env: dict[str, str]) -> None:
    if cargo_compiler_wrappers(env):
        normalized, _applied = normalize_cargo_environment(env)
        env.update(normalized)
        return
    mode = env.get("MOLT_USE_SCCACHE", "auto").strip().lower()
    if mode in {"0", "false", "no", "off"}:
        return
    forced = mode in {"1", "true", "yes", "on"}
    sccache = shutil.which("sccache")
    if sccache is None:
        if forced:
            _sccache_diag(
                "MOLT_USE_SCCACHE set but sccache is not on PATH; using direct rustc."
            )
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
        _sccache_diag(
            "server healthcheck failed; using direct rustc (set MOLT_USE_SCCACHE=0 to silence)."
        )
        return
    root = _find_molt_root(Path.cwd()) or Path.cwd()
    ext_root = Path(env.get("MOLT_EXT_ROOT", root)).expanduser()
    if not ext_root.is_absolute():
        ext_root = root / ext_root
    env.setdefault("SCCACHE_DIR", str((ext_root / ".sccache").resolve()))
    env.setdefault("SCCACHE_CACHE_SIZE", DEFAULT_SCCACHE_CACHE_SIZE)
    env["RUSTC_WRAPPER"] = sccache
    normalized, _applied = normalize_cargo_environment(env)
    env.update(normalized)
    _sccache_diag(
        f"enabled (RUSTC_WRAPPER={sccache}); post-build stats attest effectiveness."
    )


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
    # Explicit incremental policy wins only when it is compatible with the
    # active wrapper. The invariant is enforced here because every production
    # nested Cargo build obtains its environment through this authority.
    env, _applied = normalize_cargo_environment(env, default_incremental="1")
    if sys.executable:
        env.setdefault("MOLT_BUILD_PYTHON", sys.executable)
    _apply_memory_bounded_cargo_jobs(env)
    _maybe_enable_lld_link(env)
    return env


def _attest_sccache_stats(sccache: str, label: str) -> None:
    """Log post-build sccache stats so a configured-but-0-hit cache is VISIBLE
    (the 'sccache was on but doing nothing' class) instead of silently wasting
    the wrapper overhead. Cumulative counts: requests==0 after a build means the
    cache did nothing."""
    try:
        result = _run_completed_command(
            [sccache, "--show-stats"],
            cwd=Path.cwd(),
            env=os.environ.copy(),
            capture_output=True,
            memory_guard_prefix="MOLT_BUILD",
            timeout=15,
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


_SCCACHE_EXPLICIT_ERROR_RE = re.compile(
    r"(?im)^\s*(?:error:\s*)?sccache:\s*error:\s*\S"
)
_SCCACHE_LAUNCH_FAILURE_RE = re.compile(
    r"(?im)^\s*error:\s*(?:could not|failed to)\s+(?:execute|spawn)\s+"
    r"(?:process\s+)?[^\r\n]*\bsccache(?:\.exe)?\b"
)


def _sccache_wrapper_failure_reason(
    result: subprocess.CompletedProcess[Any],
) -> str | None:
    """Classify only failures that explicitly identify the wrapper as broken.

    Cargo includes the entire rustc invocation in its generic
    ``process didn't exit successfully`` diagnostic.  When ``RUSTC_WRAPPER`` is
    active that line necessarily contains ``sccache`` even when rustc itself
    failed, was killed, or exhausted memory.  Command text is therefore never
    retry authority; an explicit sccache diagnostic or wrapper launch failure is.
    """
    combined = f"{_text_output(result.stderr)}\n{_text_output(result.stdout)}"
    if _SCCACHE_EXPLICIT_ERROR_RE.search(combined):
        return "explicit-sccache-error"
    if _SCCACHE_LAUNCH_FAILURE_RE.search(combined):
        return "sccache-launch-failure"
    return None


def _cargo_attempt(
    result: subprocess.CompletedProcess[Any],
    *,
    index: int,
    wrapper: str | None,
    duration_seconds: float,
    failure_kind: str | None,
) -> CargoAttemptEvidence:
    stdout = _text_output(result.stdout)
    stderr = _text_output(result.stderr)
    elapsed = getattr(result, "elapsed_s", None)
    duration = (
        float(elapsed)
        if isinstance(elapsed, (int, float)) and elapsed >= 0
        else duration_seconds
    )
    return CargoAttemptEvidence(
        index=index,
        wrapper=wrapper,
        returncode=result.returncode,
        signal=_cargo_result_signal(result, f"{stderr}\n{stdout}"),
        timed_out=bool(getattr(result, "timed_out", False)),
        duration_seconds=max(0.0, duration),
        peak_process_rss_bytes=_rss_bytes(result, "peak"),
        peak_tree_rss_bytes=_rss_bytes(result, "peak_total"),
        failure_kind=failure_kind,
        stdout=_bounded_cargo_attempt_text(stdout),
        stderr=_bounded_cargo_attempt_text(stderr),
    )


_TempfileCargoRunner = Callable[..., subprocess.CompletedProcess[bytes]]


def _run_cargo_attempt(
    cmd: list[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    timeout: float | None,
    tempfile_runner: _TempfileCargoRunner | None,
    progress_label: str | None,
) -> subprocess.CompletedProcess[Any]:
    normalized_env, _applied = normalize_cargo_environment(env)
    if tempfile_runner is not None:
        return tempfile_runner(
            cmd,
            cwd=cwd,
            env=normalized_env,
            timeout=timeout,
            progress_label=progress_label,
        )
    return _run_completed_command(
        cmd,
        cwd=cwd,
        env=normalized_env,
        capture_output=True,
        memory_guard_prefix="MOLT_BUILD",
        timeout=timeout,
        encoding="utf-8",
        errors="strict",
    )


def _run_cargo_with_sccache_retry(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: float | None,
    json_output: bool,
    label: str,
    tempfile_runner: _TempfileCargoRunner | None = None,
    progress_label: str | None = None,
) -> CargoExecutionResult:
    started = time.perf_counter()
    build = _run_cargo_attempt(
        cmd,
        cwd=cwd,
        env=env,
        timeout=timeout,
        tempfile_runner=tempfile_runner,
        progress_label=progress_label,
    )
    first_duration = time.perf_counter() - started
    wrappers = sccache_compiler_wrappers(env)
    wrapper = wrappers[0][1] if wrappers else ""
    retry_reason = (
        _sccache_wrapper_failure_reason(build)
        if build.returncode != 0 and wrapper and _wrapper_is_sccache(wrapper)
        else None
    )
    attempts = [
        _cargo_attempt(
            build,
            index=1,
            wrapper=wrapper or None,
            duration_seconds=first_duration,
            failure_kind=retry_reason,
        )
    ]
    if retry_reason is not None:
        retry_env = without_sccache_compiler_wrappers(env)
        if not json_output:
            print(
                f"{label}: sccache wrapper failure detected ({retry_reason}); "
                "retrying once without sccache.",
                file=sys.stderr,
            )
        started = time.perf_counter()
        build = _run_cargo_attempt(
            cmd,
            cwd=cwd,
            env=retry_env,
            timeout=timeout,
            tempfile_runner=tempfile_runner,
            progress_label=progress_label,
        )
        attempts.append(
            _cargo_attempt(
                build,
                index=2,
                wrapper=None,
                duration_seconds=time.perf_counter() - started,
                failure_kind=None,
            )
        )
    active_wrappers = sccache_compiler_wrappers(env)
    active_wrapper = active_wrappers[0][1] if active_wrappers else ""
    if not json_output and active_wrapper and _wrapper_is_sccache(active_wrapper):
        _attest_sccache_stats(active_wrapper, label)
    return CargoExecutionResult(
        build,
        attempts=attempts,
        retry_reason=retry_reason,
    )


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
