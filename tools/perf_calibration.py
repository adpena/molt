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
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Callable, Optional, Sequence

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

    _kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _kernel32.OpenProcess.restype = wintypes.HANDLE
    _kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    _kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    _kernel32.GetCurrentProcess.restype = wintypes.HANDLE
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
    # AssignProcessToJobObject also needs SET_QUOTA + TERMINATE on the process.
    _PROCESS_QUERY_INFORMATION = 0x0400
    _PROCESS_VM_READ = 0x0010
    _PROCESS_SET_QUOTA = 0x0100
    _PROCESS_TERMINATE = 0x0001

    def _win_peak_wset(handle) -> Optional[int]:
        if not handle:
            return None
        pmc = _PROCESS_MEMORY_COUNTERS()
        pmc.cb = ctypes.sizeof(_PROCESS_MEMORY_COUNTERS)
        if _gpmi(handle, ctypes.byref(pmc), pmc.cb):
            return int(pmc.PeakWorkingSetSize)
        return None

    def _win_open(pid: int):
        return (
            _kernel32.OpenProcess(
                _PROCESS_QUERY_INFORMATION
                | _PROCESS_VM_READ
                | _PROCESS_SET_QUOTA
                | _PROCESS_TERMINATE,
                False,
                pid,
            )
            or None
        )

    def _win_close(handle) -> None:
        if handle:
            _kernel32.CloseHandle(handle)

    # Job object: measures the peak committed memory of the WHOLE process tree
    # (the child + every descendant, e.g. a launcher/trampoline's grandchild),
    # survives process exit, and has no polling race.
    _ULONG_PTR = ctypes.c_size_t

    class _JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
        _fields_ = [
            ("PerProcessUserTimeLimit", ctypes.c_int64),
            ("PerJobUserTimeLimit", ctypes.c_int64),
            ("LimitFlags", wintypes.DWORD),
            ("MinimumWorkingSetSize", ctypes.c_size_t),
            ("MaximumWorkingSetSize", ctypes.c_size_t),
            ("ActiveProcessLimit", wintypes.DWORD),
            ("Affinity", _ULONG_PTR),
            ("PriorityClass", wintypes.DWORD),
            ("SchedulingClass", wintypes.DWORD),
        ]

    class _IO_COUNTERS(ctypes.Structure):
        _fields_ = [
            (n, ctypes.c_uint64)
            for n in (
                "ReadOperationCount",
                "WriteOperationCount",
                "OtherOperationCount",
                "ReadTransferCount",
                "WriteTransferCount",
                "OtherTransferCount",
            )
        ]

    class _JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
        _fields_ = [
            ("BasicLimitInformation", _JOBOBJECT_BASIC_LIMIT_INFORMATION),
            ("IoInfo", _IO_COUNTERS),
            ("ProcessMemoryLimit", ctypes.c_size_t),
            ("JobMemoryLimit", ctypes.c_size_t),
            ("PeakProcessMemoryUsed", ctypes.c_size_t),
            ("PeakJobMemoryUsed", ctypes.c_size_t),
        ]

    _kernel32.CreateJobObjectW.restype = wintypes.HANDLE
    _kernel32.CreateJobObjectW.argtypes = [wintypes.LPVOID, wintypes.LPCWSTR]
    _kernel32.AssignProcessToJobObject.restype = wintypes.BOOL
    _kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    _kernel32.QueryInformationJobObject.restype = wintypes.BOOL
    _kernel32.QueryInformationJobObject.argtypes = [
        wintypes.HANDLE,
        ctypes.c_int,
        wintypes.LPVOID,
        wintypes.DWORD,
        wintypes.LPDWORD,
    ]
    _JobObjectExtendedLimitInformation = 9

    def _win_create_job():
        return _kernel32.CreateJobObjectW(None, None) or None

    def _win_assign_job(hjob, hprocess) -> bool:
        return bool(_kernel32.AssignProcessToJobObject(hjob, hprocess))

    def _win_job_peak(hjob) -> Optional[int]:
        info = _JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
        if _kernel32.QueryInformationJobObject(
            hjob,
            _JobObjectExtendedLimitInformation,
            ctypes.byref(info),
            ctypes.sizeof(info),
            None,
        ):
            return int(info.PeakJobMemoryUsed) or None
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


def _sample_peak_rss(pid: int, handle=None) -> Optional[int]:
    """One sample of a child's peak RSS so far (bytes). Linux VmHWM and Windows
    PeakWorkingSetSize are cumulative peaks (monotone), so polling captures the
    true peak; macOS samples current RSS (peak approximated by the max of samples)."""
    if sys.platform == "win32":
        return _win_peak_wset(handle)
    if sys.platform.startswith("linux"):
        try:
            with open(f"/proc/{pid}/status", "r", encoding="utf-8") as fh:
                for line in fh:
                    if line.startswith("VmHWM:"):
                        return int(line.split()[1]) * 1024  # KiB -> bytes
        except (FileNotFoundError, ProcessLookupError, ValueError, OSError):
            return None
        return None
    # macOS / other unix: sample current RSS via ps (KiB).
    try:
        out = subprocess.run(
            ["ps", "-o", "rss=", "-p", str(pid)],
            capture_output=True,
            text=True,
            timeout=2,
        ).stdout.strip()
        return int(out) * 1024 if out else None
    except (subprocess.SubprocessError, ValueError, OSError):
        return None


@dataclass
class RunMeasurement:
    returncode: int
    elapsed_s: float
    peak_rss_bytes: Optional[int]
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
) -> RunMeasurement:
    """Spawn argv, capturing wall time AND cross-platform peak RSS by polling.

    Output is captured to temp files (not pipes) so a chatty child cannot deadlock
    the poll loop by filling a pipe buffer.

    Peak RSS is SAMPLED at `poll_interval`. Real benchmark processes hold their
    working set for many intervals, so the peak is captured faithfully; a pathological
    process that allocates and exits within a single interval may under-report (the
    monotone Linux VmHWM / Windows PeakWorkingSetSize fields mitigate this, but a
    post-exit read can fail). For sub-millisecond-burst accuracy a Job Object (Windows)
    / wait4 rusage (Unix) upgrade would be needed; benchmarks do not need it."""
    full_env = None
    if env is not None:
        full_env = dict(os.environ)
        full_env.update({k: str(v) for k, v in env.items()})
    t0 = time.perf_counter()
    with tempfile.TemporaryFile() as ofh, tempfile.TemporaryFile() as efh:
        proc = subprocess.Popen(
            list(argv), stdout=ofh, stderr=efh, env=full_env, cwd=cwd
        )
        handle = _win_open(proc.pid) if sys.platform == "win32" else None
        job = _win_create_job() if sys.platform == "win32" else None
        assigned_job = (
            job
            if sys.platform == "win32"
            and handle
            and job
            and _win_assign_job(job, handle)
            else None
        )
        peak = 0
        timed_out = False
        try:
            while True:
                try:
                    proc.wait(timeout=poll_interval)
                    done = True
                except subprocess.TimeoutExpired:
                    done = False
                s = _win_job_peak(assigned_job) if assigned_job is not None else None
                if s is None:
                    s = _sample_peak_rss(proc.pid, handle)
                if s:
                    peak = max(peak, s)
                if done:
                    break
                if timeout is not None and (time.perf_counter() - t0) > timeout:
                    proc.kill()
                    proc.wait()
                    timed_out = True
                    break
            # Final sample from monotone fields. On Windows, Job peak captures
            # the child process tree even after descendants have exited.
            s = _win_job_peak(assigned_job) if assigned_job is not None else None
            if s is None:
                s = _sample_peak_rss(proc.pid, handle)
            if s:
                peak = max(peak, s)
        finally:
            if sys.platform == "win32":
                _win_close(job)
                _win_close(handle)
        elapsed = time.perf_counter() - t0
        ofh.seek(0)
        out = ofh.read().decode("utf-8", "replace")
        efh.seek(0)
        err = efh.read().decode("utf-8", "replace")
    rc = proc.returncode if proc.returncode is not None else -1
    return RunMeasurement(rc, elapsed, peak or None, out, err, timed_out)


# ---------------------------------------------------------------------------
# Quiescence (C2) -- best-effort cross-platform; NEVER fail-closed.
# ---------------------------------------------------------------------------
_COMPETING = ("cargo", "rustc", "molt-backend", "wasmtime", "node")


def _competing_build_count() -> int:
    try:
        if sys.platform == "win32":
            out = subprocess.run(
                ["tasklist", "/fo", "csv", "/nh"],
                capture_output=True,
                text=True,
                timeout=5,
            ).stdout
        else:
            out = subprocess.run(
                ["ps", "-axco", "command"], capture_output=True, text=True, timeout=5
            ).stdout
    except (subprocess.SubprocessError, OSError):
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
    """Certified iff we can MEASURE quiescence and it is quiet. On a host with no
    load probe (e.g. Windows) certified is False (we honestly cannot certify) -- the
    caller must treat 'uncertified' as: do not PROMOTE a red, but a WIN still stands."""
    try:
        load1 = os.getloadavg()[0]
    except (OSError, AttributeError):
        load1 = None
    competing = _competing_build_count()
    cores = os.cpu_count() or 1
    per_core = (load1 / cores) if load1 is not None else None
    if load1 is None:
        certified = False
        detail = "load unavailable (no os.getloadavg on this OS); uncertified"
    elif per_core is not None and per_core > max_load_per_core:
        certified = False
        detail = f"load1={load1:.2f} per_core={per_core:.2f}>{max_load_per_core}"
    else:
        certified = True
        detail = f"load1={load1:.2f} per_core={per_core:.2f}"
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

    mx = ordered[-1] if ordered else None
    fp = host_fingerprint()
    return {
        "kind": "cold_budget_calibration",
        "runs": runs,
        "measured_p50_ms": round(pct(50), 2) if ordered else None,
        "measured_p90_ms": round(pct(90), 2) if ordered else None,
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
