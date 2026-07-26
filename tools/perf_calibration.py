#!/usr/bin/env python3
"""Dynamic, host-aware, cross-platform calibration for the molt perf scoreboard.

The C1/C2/C3/C4 substrate of
docs/design/foundation/69_benchmark_corpus_union_and_dynamic_calibration.md.
It makes every benchmark cell trustworthy across OS / arch / Python-version:

  - host_fingerprint()      : identity that keys all calibration (C1/C5)
  - measure_quiescence()    : cross-platform load probe; best-effort, NEVER
                              fail-closed. Gate a RED-promotion on it, never a WIN
                              (load can only slow molt, so a win under load is
                              conservative) (C2)
  - peak_rss_self_bytes()   : peak RSS of the current process, any OS
  - run_and_measure()       : spawn a child and capture wall time AND uniform
                              cross-platform peak RSS, fixing the Windows "RSS=0"
                              gap the native board hit (C4)
  - adaptive_samples()      : pyperf-grade adaptive sampling + 95% CI; resolve
                              UNSTABLE by sampling more, report median+CI+CV (C3)
  - calibrate_cold_budget() : per-host cold-start budget, replacing the static
                              macOS-seeded constant (v0 = measured baseline, per
                              host) (C1)

No third-party dependency: psutil is intentionally NOT used (consistent with the
repo's existing memory-guard tooling and a zero-install posture) -- stdlib + ctypes
only, so calibration runs identically on every host molt targets.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import math
import os
import platform
import statistics
import sys
import time
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path
from subprocess import SubprocessError
from typing import Any, Callable, Optional, Sequence

try:
    from tools import harness_memory_guard
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    import harness_memory_guard
    from command_execution import CommandExecutor

_COMMANDS = CommandExecutor.for_file(__file__)

# ---------------------------------------------------------------------------
# Windows peak-working-set via ctypes (macOS/Linux use stdlib only).
# ---------------------------------------------------------------------------
if sys.platform == "win32":
    from ctypes import wintypes

    class _PROCESS_MEMORY_COUNTERS(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("PageFaultCount", wintypes.DWORD),
            ("PeakWorkingSetSize", ctypes.c_size_t),
            ("WorkingSetSize", ctypes.c_size_t),
            ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
            ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
            ("PagefileUsage", ctypes.c_size_t),
            ("PeakPagefileUsage", ctypes.c_size_t),
        ]

    class _FILETIME(ctypes.Structure):
        _fields_ = [("low", wintypes.DWORD), ("high", wintypes.DWORD)]

    _kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _kernel32.OpenProcess.restype = wintypes.HANDLE
    _kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    _kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    _kernel32.GetCurrentProcess.restype = wintypes.HANDLE
    _kernel32.GetSystemTimes.argtypes = [
        ctypes.POINTER(_FILETIME),
        ctypes.POINTER(_FILETIME),
        ctypes.POINTER(_FILETIME),
    ]
    _kernel32.GetSystemTimes.restype = wintypes.BOOL
    # GetProcessMemoryInfo lives in psapi.dll; modern Windows also exports the
    # K32-prefixed alias from kernel32.
    try:
        _gpmi = ctypes.WinDLL("psapi", use_last_error=True).GetProcessMemoryInfo
    except (OSError, AttributeError):
        _gpmi = _kernel32.K32GetProcessMemoryInfo
    _gpmi.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(_PROCESS_MEMORY_COUNTERS),
        wintypes.DWORD,
    ]
    _gpmi.restype = wintypes.BOOL

    # GetProcessMemoryInfo classically needs QUERY_INFORMATION + VM_READ.
    _PROCESS_QUERY_INFORMATION = 0x0400
    _PROCESS_VM_READ = 0x0010

    def _win_peak_wset(handle) -> Optional[int]:
        if not handle:
            return None
        pmc = _PROCESS_MEMORY_COUNTERS()
        pmc.cb = ctypes.sizeof(_PROCESS_MEMORY_COUNTERS)
        if _gpmi(handle, ctypes.byref(pmc), pmc.cb):
            return int(pmc.PeakWorkingSetSize)
        return None


# ---------------------------------------------------------------------------
# Host fingerprint (C1/C5) -- keys every calibration artifact.
# ---------------------------------------------------------------------------
@dataclass(frozen=True)
class HostFingerprint:
    os: str
    arch: str
    cpu: str
    logical_cores: int
    python_version: str

    def key(self) -> str:
        raw = f"{self.os}|{self.arch}|{self.cpu}|{self.logical_cores}|{self.python_version}"
        return hashlib.sha1(raw.encode("utf-8")).hexdigest()[:16]


def host_fingerprint() -> HostFingerprint:
    return HostFingerprint(
        os=platform.system() or sys.platform,
        arch=platform.machine() or "unknown",
        cpu=(platform.processor() or platform.machine() or "unknown"),
        logical_cores=os.cpu_count() or 1,
        python_version=platform.python_version(),
    )


# ---------------------------------------------------------------------------
# Cross-platform peak RSS (C4) -- fixes the Windows RSS=0 gap.
# ---------------------------------------------------------------------------
def peak_rss_self_bytes() -> Optional[int]:
    """Peak resident set of the CURRENT process, in bytes, on any OS."""
    if sys.platform == "win32":
        return _win_peak_wset(_kernel32.GetCurrentProcess())
    try:
        import resource
    except ImportError:
        return None
    maxrss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    # Linux reports KiB; macOS/BSD report bytes.
    return maxrss * 1024 if sys.platform.startswith("linux") else maxrss


@dataclass
class RunMeasurement:
    returncode: int
    elapsed_s: float
    peak_rss_bytes: Optional[int]
    peak_job_commit_bytes: Optional[int]
    stdout: str
    stderr: str
    timed_out: bool = False


def run_and_measure(
    argv: Sequence[str],
    *,
    timeout: Optional[float] = None,
    env: Optional[dict] = None,
    cwd: Optional[str] = None,
    poll_interval: float = 0.003,
    on_spawn: Optional[Callable[[int], None]] = None,
) -> RunMeasurement:
    """Run one benchmark under the repository's process-tree custody authority."""

    if poll_interval <= 0:
        raise ValueError("poll_interval must be positive")
    full_env = dict(os.environ)
    if env is not None:
        full_env.update({key: str(value) for key, value in env.items()})
    context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_PERF_CALIBRATION",
        full_env,
        repo_root=Path(__file__).resolve().parents[1],
    )
    context = replace(
        context,
        limits=replace(context.limits, poll_interval=poll_interval),
    )
    result = context.run(
        [str(part) for part in argv],
        cwd=cwd,
        capture_output=True,
        text=False,
        timeout=timeout,
        on_spawn=on_spawn,
        sampling_scope="owned_tree",
    )
    if result.elapsed_s is None:
        raise RuntimeError("guarded benchmark returned without elapsed-time telemetry")
    peak = result.peak_total
    peak_rss_bytes = None if peak is None else peak.rss_kb * 1024

    def decode_output(value: object) -> str:
        if value is None:
            return ""
        if isinstance(value, bytes):
            return value.decode("utf-8", "replace")
        if isinstance(value, str):
            return value
        raise TypeError(
            f"guarded benchmark returned invalid output type {type(value)!r}"
        )

    out = decode_output(result.stdout)
    err = decode_output(result.stderr)
    return RunMeasurement(
        result.returncode,
        result.elapsed_s,
        peak_rss_bytes,
        result.peak_job_commit_bytes,
        out,
        err,
        result.timed_out,
    )


# ---------------------------------------------------------------------------
# Quiescence (C2) -- best-effort cross-platform; NEVER fail-closed.
# ---------------------------------------------------------------------------
_COMPETING = ("cargo", "rustc", "molt-backend", "wasmtime")


def _filetime_ticks(value: Any) -> int:
    return (int(value.high) << 32) | int(value.low)


def _windows_cpu_load(sample_seconds: float = 0.25) -> Optional[float]:
    """Return whole-host CPU utilization as a fraction using GetSystemTimes."""
    if sys.platform != "win32":
        return None

    def snapshot() -> tuple[int, int, int] | None:
        idle = _FILETIME()
        kernel = _FILETIME()
        user = _FILETIME()
        if not _kernel32.GetSystemTimes(
            ctypes.byref(idle), ctypes.byref(kernel), ctypes.byref(user)
        ):
            return None
        return (
            _filetime_ticks(idle),
            _filetime_ticks(kernel),
            _filetime_ticks(user),
        )

    before = snapshot()
    if before is None:
        return None
    time.sleep(sample_seconds)
    after = snapshot()
    if after is None:
        return None
    idle_delta = after[0] - before[0]
    total_delta = (after[1] - before[1]) + (after[2] - before[2])
    if total_delta <= 0 or idle_delta < 0:
        return None
    return min(1.0, max(0.0, (total_delta - idle_delta) / total_delta))


def _competing_build_count() -> int:
    try:
        command = (
            ["tasklist", "/fo", "csv", "/nh"]
            if sys.platform == "win32"
            else ["ps", "-axco", "command"]
        )
        out = _COMMANDS.run(
            command,
            capture_output=True,
            text=True,
            timeout=5,
            check=True,
        ).stdout
    except (OSError, SubprocessError, ValueError):
        return -1
    if not isinstance(out, str):
        return -1
    low = out.lower()
    return sum(low.count(n) for n in _COMPETING)


@dataclass
class Quiescence:
    certified: bool
    load1: Optional[float]
    load_per_core: Optional[float]
    competing_builds: int
    detail: str


def measure_quiescence(max_load_per_core: float = 0.35) -> Quiescence:
    """Certified iff whole-host load is measured and below the policy ceiling."""
    windows_cpu = _windows_cpu_load() if sys.platform == "win32" else None
    if windows_cpu is not None:
        cores = os.cpu_count() or 1
        load1 = windows_cpu * cores
        probe = "GetSystemTimes"
    else:
        getloadavg = getattr(os, "getloadavg", None)
        try:
            load1 = None if getloadavg is None else getloadavg()[0]
        except OSError:
            load1 = None
        cores = os.cpu_count() or 1
        probe = "getloadavg"
    competing = _competing_build_count()
    per_core = (load1 / cores) if load1 is not None else None
    if load1 is None:
        certified = False
        detail = "whole-host load unavailable; uncertified"
    elif per_core is not None and per_core > max_load_per_core:
        certified = False
        detail = (
            f"{probe} active_cores={load1:.2f} "
            f"per_core={per_core:.2f}>{max_load_per_core}"
        )
    else:
        certified = True
        detail = f"{probe} active_cores={load1:.2f} per_core={per_core:.2f}"
    if competing > 0:
        detail += f"; competing~{competing}"
    return Quiescence(certified, load1, per_core, max(competing, 0), detail)


# ---------------------------------------------------------------------------
# Adaptive sampling + confidence interval (C3) -- pyperf-grade.
# ---------------------------------------------------------------------------
@dataclass
class SampleStats:
    n: int
    median: float
    mean: float
    stdev: float
    cv: float
    ci95_low: float
    ci95_high: float
    ci95_rel_halfwidth: float
    converged: bool
    samples: list = field(default_factory=list)


def _summarize(xs: Sequence[float]) -> SampleStats:
    n = len(xs)
    med = statistics.median(xs)
    mean = statistics.fmean(xs)
    sd = statistics.stdev(xs) if n > 1 else 0.0
    half = (
        1.96 * sd / math.sqrt(n) if n > 1 else 0.0
    )  # 95% CI of the mean (normal approx)
    rel = (half / mean) if mean else 0.0
    cv = (sd / mean) if mean else 0.0
    return SampleStats(
        n, med, mean, sd, cv, mean - half, mean + half, rel, False, list(xs)
    )


def adaptive_samples(
    measure: Callable[[], float],
    *,
    min_n: int = 5,
    max_n: int = 50,
    target_rel_ci: float = 0.02,
    warmup: int = 1,
) -> SampleStats:
    """Run measure() (returns one timing) until the 95% CI half-width is within
    target_rel_ci of the mean, or max_n samples. Discards `warmup` runs first."""
    for _ in range(max(0, warmup)):
        measure()
    xs = [float(measure()) for _ in range(max(1, min_n))]
    stats = _summarize(xs)
    while stats.n < max_n and stats.ci95_rel_halfwidth > target_rel_ci:
        xs.append(float(measure()))
        stats = _summarize(xs)
    stats.converged = stats.ci95_rel_halfwidth <= target_rel_ci
    return stats


# ---------------------------------------------------------------------------
# Host-keyed calibration cache + cold-start budget calibration (C1).
# ---------------------------------------------------------------------------
def calibration_root(repo_root: Optional[Path] = None) -> Path:
    root = Path(repo_root) if repo_root else Path(__file__).resolve().parents[1]
    return root / "bench" / "scoreboard" / "host_calibration"


def save_calibration(
    data: dict, *, fp: Optional[HostFingerprint] = None, repo_root=None
) -> Path:
    fp = fp or host_fingerprint()
    d = calibration_root(repo_root)
    d.mkdir(parents=True, exist_ok=True)
    path = d / f"{fp.key()}.json"
    payload = {
        "fingerprint": asdict(fp),
        "fingerprint_key": fp.key(),
        "calibration": data,
    }
    path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    return path


def load_calibration(
    *, fp: Optional[HostFingerprint] = None, repo_root=None
) -> Optional[dict]:
    fp = fp or host_fingerprint()
    path = calibration_root(repo_root) / f"{fp.key()}.json"
    if not path.exists():
        return None
    return json.loads(path.read_text(encoding="utf-8"))


def calibrate_cold_budget(
    run_argv: Sequence[str],
    *,
    runs: int = 11,
    margin_frac: float = 0.15,
    env=None,
    cwd=None,
) -> dict:
    """Measure this host's cold-start floor for run_argv (a minimal program) and
    derive a budget. v0 = measured baseline (council ruling A), PER HOST. The budget
    bounds the FIRST-RUN tax so the gate does not regress from the host's own floor."""
    samples_ms: list[float] = []
    rss: list[int] = []
    for _ in range(max(1, runs)):
        m = run_and_measure(run_argv, env=env, cwd=cwd)
        samples_ms.append(m.elapsed_s * 1000.0)
        if m.peak_rss_bytes:
            rss.append(m.peak_rss_bytes)
    ordered = sorted(samples_ms)

    def pct(p: float) -> Optional[float]:
        if not ordered:
            return None
        k = min(len(ordered) - 1, int(round((p / 100.0) * (len(ordered) - 1))))
        return ordered[k]

    p50 = pct(50)
    p90 = pct(90)
    mx = ordered[-1] if ordered else None
    fp = host_fingerprint()
    return {
        "kind": "cold_budget_calibration",
        "runs": runs,
        "measured_p50_ms": round(p50, 2) if p50 is not None else None,
        "measured_p90_ms": round(p90, 2) if p90 is not None else None,
        "measured_max_ms": round(mx, 2) if mx else None,
        "budget_ms": round(mx * (1.0 + margin_frac)) if mx else None,
        "margin_frac": margin_frac,
        "peak_rss_bytes_max": max(rss) if rss else None,
        "host_arch": fp.arch,
        "host_os": fp.os,
    }


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def _selftest() -> int:
    fp = host_fingerprint()
    print(
        f"[fingerprint] {fp.os}/{fp.arch} cores={fp.logical_cores} py={fp.python_version} key={fp.key()}"
    )
    self_rss = peak_rss_self_bytes()
    print(f"[peak_rss_self] {self_rss} bytes ({'OK' if self_rss else 'UNAVAILABLE'})")
    m = run_and_measure(
        [sys.executable, "-c", "x=bytearray(40_000_000); print(len(x))"]
    )
    print(
        f"[run_and_measure] rc={m.returncode} elapsed={m.elapsed_s * 1000:.1f}ms peak_rss={m.peak_rss_bytes} out={m.stdout.strip()!r}"
    )
    q = measure_quiescence()
    print(f"[quiescence] certified={q.certified} {q.detail}")
    base = time.perf_counter()
    s = adaptive_samples(
        lambda: time.perf_counter() - base + 1.0,
        min_n=5,
        max_n=20,
        target_rel_ci=0.5,
        warmup=0,
    )
    print(
        f"[adaptive] n={s.n} median={s.median:.4f} cv={s.cv:.4f} converged={s.converged}"
    )
    ok = bool(self_rss) and m.returncode == 0 and (m.peak_rss_bytes or 0) > 10_000_000
    print(f"[selftest] {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def _main(argv=None) -> int:
    ap = argparse.ArgumentParser(
        description="Dynamic cross-platform perf calibration (doc 69 C1-C4)."
    )
    sub = ap.add_subparsers(dest="command", required=True)
    sub.add_parser("fingerprint")
    sub.add_parser("quiescence")
    sub.add_parser("selftest")
    cb = sub.add_parser("cold-budget")
    cb.add_argument("--runs", type=int, default=11)
    cb.add_argument("--save", action="store_true")
    cb.add_argument(
        "run_argv", nargs=argparse.REMAINDER, help="-- <argv of a minimal program>"
    )
    args = ap.parse_args(argv)
    if args.command == "fingerprint":
        fp = host_fingerprint()
        print(json.dumps({**asdict(fp), "key": fp.key()}, indent=2))
    elif args.command == "quiescence":
        print(json.dumps(asdict(measure_quiescence()), indent=2))
    elif args.command == "selftest":
        return _selftest()
    elif args.command == "cold-budget":
        cmd = [a for a in args.run_argv if a != "--"]
        if not cmd:
            cmd = [sys.executable, "-c", "pass"]
        result = calibrate_cold_budget(cmd, runs=args.runs)
        if args.save:
            result["saved_to"] = str(save_calibration(result))
        print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
