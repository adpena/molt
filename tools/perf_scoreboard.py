#!/usr/bin/env python3
"""CPython floor-scoreboard — the release-blocking performance gate.

Operationalizes the molt **Performance Constitution** (CLAUDE.md, commit
538f4386e): CPython is the absolute floor. Any benchmark whose molt SPEEDUP
(``cpython_time / molt_time``) is below ``1.00`` is **RED** — a contract
violation, not "later optimization work."

This tool MEASURES and SURFACES; it does not fix slow benchmarks. It emits a
single machine-readable scoreboard keyed ``benchmark x target x backend x
profile`` reporting the absolute molt/CPython speedup, binary size, peak RSS,
compile time, and cold + warm ratios with a stability flag, and exits nonzero
if ANY cell is RED or unstable-unmeasurable (CI-gateable).

Direction (LABELLED unambiguously): ``speedup = cpython_time / molt_time``.
  * ``speedup > 1.0``  -> molt is FASTER than CPython (GREEN).
  * ``speedup < 1.0``  -> molt is SLOWER than CPython (RED — contract violation).

Reuse posture
-------------
The canonical molt-vs-CPython wall-time machinery already lives in
``tools/bench.py`` (daemon batch build with the harness memory guard,
binary-size + compile-time capture, the curated suite in
``tools/bench_suites.py``). We REUSE it for building. Every benchmark *run*
(molt binary AND the CPython baseline) is timed through ``tools/safe_run.py
--json``, which enforces the RSS cap + wall-clock timeout the project mandates
for raw-binary execution and reports ``peak_rss_mib`` + ``elapsed_s`` for both
runtimes — satisfying the constitution's peak-RSS column for free.

Methodology (pyperf discipline)
-------------------------------
  * >= 5 measured samples per (benchmark, runtime) phase (configurable).
  * median + stdev + coefficient-of-variation outlier/instability detection.
  * COLD (first cold-cache run) AND WARM (steady-state) both captured — the
    constitution forbids warm-only wins.
  * MOLT_SESSION_ID=perfscore, CARGO_TARGET_DIR=target/sessions/perfscore.

Backends / profiles (baseline run)
----------------------------------
  * native + llvm. WASM is build/link-only today (a known socket-import
    instantiation gap blocks its run-path) -> recorded as ``run-blocked``.
    Luau lives in ``tools/benchmark_luau_vs_cpython.py`` -> follow-up.
  * release-fast is the daily-contract profile (``--build-profile release`` in
    the CLI maps to the ``release-fast`` cargo profile for the backend).
    release-output is the incremental next fill.

PyPy / Codon columns are present-but-nullable in the schema; neither is
installed on this host. See ``docs/perf/SCOREBOARD.md`` for the toolchain arc.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = REPO_ROOT / "tools"
SRC_ROOT = REPO_ROOT / "src"

if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

# Reuse the canonical build + suite machinery rather than rebuilding a timing
# loop. bench.py owns the daemon batch build, the memory guard, and the
# binary-size / compile-time capture; bench_suites owns the curated suite.
import bench  # noqa: E402
import bench_suites  # noqa: E402
import harness_memory_guard  # noqa: E402
import perf_inner_repeat  # noqa: E402  (#76 inner-repeat transform)
from perf_schema import (  # noqa: E402
    CLASS_DIMENSIONAL_WIN,
    CLASS_GREEN,
    CLASS_INFRA,
    CLASS_RED_NOISY,
    CLASS_RED_STABLE,
    CLASS_TIE,
    CLASSIFY_STATES as CLASSIFY_STATES,
    GATE_FAILING_VERDICTS,
    RED_THRESHOLD,
    SCHEMA_VERSION,
    UNSTABLE_CV,
    VERDICT_BUILD_FAILED,
    VERDICT_CPY_INCOMPAT,
    VERDICT_FAIL_COLD_BUDGET,
    VERDICT_FAIL_ENGINE,
    VERDICT_FAIL_STALE,
    VERDICT_GREEN,
    VERDICT_RUN_BLOCKED,
    VERDICT_RUN_ERROR,
    VERDICT_UNSTABLE,
    VERDICT_WARN_COLD_FLOOR,
    flatten_cells,
    validate_board,
    verdict_fails_gate,
)

SAFE_RUN = TOOLS_ROOT / "safe_run.py"
SCOREBOARD_DIR = REPO_ROOT / "bench" / "scoreboard"
COLD_START_BUDGET_PATH = SCOREBOARD_DIR / "cold_start_budget.json"

# The constitution's session isolation (must be set before any build command).
PERFSCORE_SESSION_ID = "perfscore"

# Verdict/classification vocabularies and gate thresholds live in
# perf_schema.py so board projections and gates share one JSON contract
# authority.

# --- Quiescence-guard thresholds (#69 Rule 2) -------------------------------
# A run is AUTHORITATIVE only on a quiet machine. The dominant contamination
# mode the council caught: a "0.66 red" that was a loaded-machine artifact (a
# parallel multi-agent build stole cycles from the timed process). We refuse-or-
# downgrade authority when ANY of these hold. Codex is NEVER counted (project
# policy: never count/kill codex) — it is filtered out of the build detector.
#
# Load threshold = ncpu * QUIESCENT_LOAD_FRACTION. On an 18-core host that is
# load > 9.0; the timing host idles ~3, a single parallel cargo build pushes it
# well past 9. The fraction is deliberately permissive (0.5) so a quiet machine
# with normal desktop background load still measures, while an active build is
# always caught (a build also trips the process check directly).
QUIESCENT_LOAD_FRACTION = 0.5
# Process-name patterns that mean "build/test work is competing for cycles".
# Matched against the FULL command line (pgrep -fl). Codex is excluded by name.
_BUILD_PROC_PATTERNS = ("cargo", "rustc", "molt-backend", "molt build")
_CODEX_EXCLUDE = "codex"  # never counted/killed (project policy)
# Dimensional-win materiality gate: a non-warm-flip improvement must beat the
# baseline by at least this fraction on a dimension (alloc/RSS/size/cold) to be
# called a DIMENSIONAL_WIN rather than noise. 5% is the smallest delta that
# survives run-to-run measurement jitter on these dimensions.
DIMENSIONAL_WIN_MIN_FRACTION = 0.05

# --- Warm-hot cycle-attribution machinery (#76) -----------------------------
# A one-shot benchmark binary spends ~85-92% of leaf self-time in process
# launch + first-touch page-in (``_dyld_start``), so the steady-state Python
# hot path never dominates a CPU sample (measured: #69 quiet board). The fix is
# two-part: (1) INNER-REPEAT — wrap the benchmark body in ``for _ in range(N)``
# inside ONE process so launch amortizes (pyperf's inner_loops model); and
# (2) SYMBOLICATE — build with MOLT_KEEP_SYMBOLS=1 so the linker keeps molt
# user-fn / runtime symbol names (release default strips them via -Wl,-x -Wl,-S
# + a post-link `strip -x`), letting /usr/bin/sample attribute to real
# functions instead of ``???``.
#
# MOLT_KEEP_SYMBOLS=1 is molt's EXISTING diagnostic-only build env hatch
# (src/molt/cli.py): it skips BOTH the link-time strip and the post-link strip,
# keeping local symbol names. It never changes default product output and adds
# only a symbol table (the CODE is byte-identical to the stripped build, so the
# profiling binary's TIMING is representative). We REUSE it rather than add a
# redundant cargo profile — the user-fn symbols come from the final-link strip,
# which a [profile.*] would not govern (those symbols live in the Cranelift
# `output.o`, already unstripped pre-link).
MOLT_KEEP_SYMBOLS_ENV = "MOLT_KEEP_SYMBOLS"
# Default inner-repeat factor when --sample-hot-only is requested without an
# explicit --inner-repeat. 40 amortizes a ~0.6s launch over dozens of
# steady-state bodies (a multi-second process for the curated benchmarks) while
# staying inside a sane RSS budget. The refusal gate below enforces that this
# was enough (raise it if launch still dominates); if a benchmark amplifies a
# per-iteration leak past the RSS cap at this N, the OOM-refusal says to LOWER
# it. (Tuned: at N=40 bench_etl_orders ~2.6s/2.0GiB, bench_exception_heavy
# ~3.8s; both yield launch=0% in-binary=100% steady-state leaderboards.)
DEFAULT_INNER_REPEAT = 40
# The CPU sample warmup (seconds) BEFORE the sampler attaches, so the first
# iterations (cold I-cache, first-touch page-in) are not in the steady window.
HOT_SAMPLE_WARMUP_S = 0.6
# The steady-state sampling window (seconds); auto-fitted DOWN to the looped
# process's remaining lifetime so the sampler always closes before exit.
HOT_SAMPLE_WINDOW_S = 3.0
# REFUSAL gate (same fail-closed discipline as #69's quiescence guard): if
# launch/page-in still accounts for >= this fraction of leaf self-time AFTER
# inner-repeat + symbols, the loop factor was too small — the attribution is
# INVALID and the tool refuses to emit a hot-path claim ("increase
# --inner-repeat") rather than report a launch-dominated leaderboard as if it
# were the program's hot path.
LAUNCH_DOMINANCE_REFUSAL_FRACTION = 0.40
# Leaf-self-time frames that are PROCESS LAUNCH / PAGE-IN, not program work.
# ``_dyld_start`` is the dynamic-loader entry (launch + first-touch page-in of
# the static binary); the dyld TLV bootstrap and the launch-time image-load
# msg traps are the same class. Matched (symbol, lib) so a same-named program
# symbol is never mis-counted as launch.
_LAUNCH_FRAMES = (
    ("_dyld_start", "dyld"),
    ("__dyld_start", "dyld"),
)

# Notes that are DERIVED from a verdict (vs measurement notes). A re-derive
# (rebuild-summary/merge) clears these so a stale verdict-note can't leak.
_VERDICT_DERIVED_NOTES = frozenset(
    {
        "non-authoritative tree (local != origin/main or dirty)",
    }
)


class ScoreboardSchemaError(RuntimeError):
    """Raised when a CPython scoreboard document violates schema authority."""

    def __init__(self, context: str, problems: list[str]) -> None:
        self.context = context
        self.problems = problems
        detail = "; ".join(problems)
        super().__init__(f"{context}: {detail}")


def _load_cold_start_budgets() -> dict:
    """Load the per (backend, profile) cold-start tax budgets in milliseconds.

    Shape: ``{"budgets": {"native/release-fast": {"budget_ms": N, ...}}}``.
    A missing file or missing cell entry means "no budget recorded yet" — the
    FAIL_COLD_BUDGET verdict cannot fire (we never invent a budget), and the
    board records the measured tax so the budget can be seeded from this run.
    """
    if not COLD_START_BUDGET_PATH.exists():
        return {"budgets": {}}
    try:
        return json.loads(COLD_START_BUDGET_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {"budgets": {}}


def _budget_ms_for(budgets: dict, backend: str, profile: str) -> float | None:
    entry = budgets.get("budgets", {}).get(f"{backend}/{profile}")
    if not isinstance(entry, dict):
        return None
    val = entry.get("budget_ms")
    return float(val) if isinstance(val, (int, float)) else None


# safe_run RSS cap + wall-clock timeout per run. Generous enough for the heavy
# benchmarks (class_hierarchy, bytes_find @ 2s CPython) without letting a
# runaway reach OOM territory.
DEFAULT_RUN_RSS_MB = 4096
DEFAULT_RUN_TIMEOUT_S = 120.0
# Tight RSS poll so short benchmarks still capture a representative peak.
SAFE_RUN_POLL_S = 0.01

DEFAULT_SAMPLES = 5
DEFAULT_WARMUP = 2

# WASM cannot be run on this host today (socket-import instantiation gap); we
# build+link only and mark the run-path blocked. Luau has its own harness.
RUN_BLOCKED_BACKENDS = {"wasm"}


# ---------------------------------------------------------------------------
# Backend / profile descriptors
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class BackendSpec:
    """A (target, backend) build lane.

    ``molt_backend`` is the value forced into ``MOLT_BACKEND`` so the daemon
    selects the right codegen (native Cranelift vs the inkwell/LLVM feature).
    ``build_target`` is the CLI ``--target`` (native vs wasm).
    """

    target: str  # logical target name in the scoreboard ("native", "wasm")
    backend: str  # codegen backend ("native", "llvm", "wasm")
    molt_backend: str | None  # MOLT_BACKEND env value, or None to leave unset
    build_target: str  # molt CLI --target


NATIVE_CRANELIFT = BackendSpec("native", "native", None, "native")
NATIVE_LLVM = BackendSpec("native", "llvm", "llvm", "native")
WASM = BackendSpec("wasm", "wasm", None, "wasm")

BACKENDS_BY_NAME = {
    "native": NATIVE_CRANELIFT,
    "llvm": NATIVE_LLVM,
    "wasm": WASM,
}

# CLI --build-profile value -> the cargo profile it resolves to (for the
# scoreboard label). "release" is the daily contract profile and maps to the
# release-fast cargo profile for the backend (see cli.py:_backend_profile).
PROFILE_BUILD_FLAG = {
    "release-fast": "release",
    "release-output": "release",  # same CLI flag; distinguished by env below
    "dev-fast": "dev",
}


def _llvm_sys_prefix() -> str | None:
    """Resolve the LLVM_SYS prefix the inkwell backend build needs.

    The brew default is llvm@22 which is the WRONG version for this tree
    (llvm-sys 211 expects LLVM 21). Prefer an already-exported value, else
    fall back to the canonical llvm@21 cellar path.
    """
    explicit = os.environ.get("LLVM_SYS_211_PREFIX", "").strip()
    if explicit:
        return explicit
    candidate = Path("/opt/homebrew/opt/llvm@21")
    if candidate.exists():
        return str(candidate)
    return None


# ---------------------------------------------------------------------------
# Run timing via safe_run.py (RSS cap + timeout + peak-RSS, for EVERY binary)
# ---------------------------------------------------------------------------


@dataclass
class RunOutcome:
    ok: bool
    elapsed_s: float | None
    peak_rss_mib: float | None
    status: str  # "ok" | "timeout" | "oom" | "error" | "nonzero"
    exit_code: int | None
    stdout: str | None = None
    stdout_tail: str | None = None
    stderr_tail: str | None = None


def _tail_text(text: str | None, *, max_chars: int = 4096) -> str | None:
    if not text:
        return None
    if len(text) <= max_chars:
        return text
    return text[-max_chars:]


def _safe_run_json(
    cmd: list[str],
    *,
    env: dict[str, str],
    rss_mb: int,
    timeout_s: float,
    label: str,
    capture_stdout: bool = False,
) -> RunOutcome:
    """Time one process through safe_run.py --json (RSS cap + timeout).

    safe_run forwards the child's stdout live and reports status as a single
    ``SAFE_RUN {json}`` line on stderr. We parse that line for elapsed_s +
    peak_rss_mib. When ``capture_stdout`` is set the child's stdout is captured
    here (for the one-time output-parity sanity); the repeated timed runs leave
    it streaming so safe_run's accounting stays honest.
    """
    full = [
        sys.executable,
        str(SAFE_RUN),
        "--json",
        "--rss-mb",
        str(rss_mb),
        "--timeout",
        str(timeout_s),
        "--poll",
        str(SAFE_RUN_POLL_S),
        "--label",
        label,
        "--",
        *cmd,
    ]
    proc = harness_memory_guard.guarded_completed_process(
        full,
        prefix="MOLT_BENCH",
        env=env,
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=timeout_s + 30.0,
    )
    if proc.timed_out:
        return RunOutcome(
            False,
            None,
            None,
            "timeout",
            None,
            stdout_tail=_tail_text(proc.stdout),
            stderr_tail=_tail_text(proc.stderr),
        )

    payload = _parse_safe_run_line(proc.stderr or "")
    if payload is None:
        return RunOutcome(
            False,
            None,
            None,
            "error",
            proc.returncode,
            stdout_tail=_tail_text(proc.stdout),
            stderr_tail=_tail_text(proc.stderr),
        )
    status = payload.get("status", "error")
    elapsed = payload.get("elapsed_s")
    peak = payload.get("peak_rss_mib")
    exit_code = payload.get("exit")
    ok = status == "ok" and isinstance(exit_code, int) and exit_code == 0
    if not ok and status == "ok":
        status = "nonzero"
    return RunOutcome(
        ok=bool(ok),
        elapsed_s=float(elapsed) if isinstance(elapsed, (int, float)) else None,
        peak_rss_mib=float(peak) if isinstance(peak, (int, float)) else None,
        status=status,
        exit_code=exit_code if isinstance(exit_code, int) else None,
        stdout=proc.stdout if capture_stdout else None,
        stdout_tail=_tail_text(proc.stdout),
        stderr_tail=_tail_text(proc.stderr),
    )


def _parse_safe_run_line(stderr_text: str) -> dict | None:
    for line in reversed(stderr_text.splitlines()):
        line = line.strip()
        if line.startswith("SAFE_RUN ") and line[9:].lstrip().startswith("{"):
            try:
                return json.loads(line[9:].lstrip())
            except json.JSONDecodeError:
                continue
    return None


# ---------------------------------------------------------------------------
# Statistics — median, stdev, coefficient of variation, stability
# ---------------------------------------------------------------------------


@dataclass
class PhaseStats:
    samples_s: list[float] = field(default_factory=list)
    median_s: float | None = None
    mean_s: float | None = None
    stdev_s: float | None = None
    cv: float | None = None  # coefficient of variation (stdev / median)
    min_s: float | None = None
    max_s: float | None = None
    peak_rss_mib: float | None = None
    stable: bool = False
    n: int = 0

    @classmethod
    def from_runs(cls, runs: list[RunOutcome]) -> "PhaseStats":
        oks = [r for r in runs if r.ok and r.elapsed_s is not None]
        samples = [r.elapsed_s for r in oks if r.elapsed_s is not None]
        peaks = [r.peak_rss_mib for r in oks if r.peak_rss_mib is not None]
        if not samples:
            return cls(samples_s=[], n=0, stable=False)
        median = statistics.median(samples)
        mean = statistics.mean(samples)
        stdev = statistics.stdev(samples) if len(samples) > 1 else 0.0
        cv = (stdev / median) if median > 0 else None
        stable = (cv is not None and cv <= UNSTABLE_CV) and len(samples) >= 2
        return cls(
            samples_s=[round(s, 6) for s in samples],
            median_s=round(median, 6),
            mean_s=round(mean, 6),
            stdev_s=round(stdev, 6),
            cv=round(cv, 4) if cv is not None else None,
            min_s=round(min(samples), 6),
            max_s=round(max(samples), 6),
            peak_rss_mib=round(max(peaks), 1) if peaks else None,
            stable=stable,
            n=len(samples),
        )


def _robust_cell_stable(molt: PhaseStats, cpy: PhaseStats) -> bool:
    """Is the cell's warm verdict trustworthy despite CPython-side outliers?

    molt is the artifact under test and MUST be stable. CPython is the reference
    floor; a single CPython GC/scheduler spike (or one anomalously-fast
    cold-cache sample) must not throw out a cell where molt wins decisively and
    is itself stable. The cell is stable iff:
      * molt is stable (low CV), AND
      * EITHER CPython is also stable,
        OR the warm verdict is robust to CPython's spread WITH the single most
        extreme sample on each side TRIMMED — i.e. the warm_speedup
        (cpython/molt, on the molt median) keeps the same side of the 1.00 floor
        whether computed with CPython's 2nd-fastest or 2nd-slowest sample. The
        trim makes the bound robust to exactly one outlier (the dominant failure
        mode: one GC spike OR one fast cold-cache run) rather than using the raw
        min/max, which a lone outlier drags to the flip point. This is a
        median-of-bounds robustness test per pyperf discipline, never a per-test
        special case.
    """
    if not molt.stable:
        return False
    if cpy.stable:
        return True
    if molt.median_s is None or molt.median_s <= 0:
        return False
    samples = sorted(cpy.samples_s or [])
    if len(samples) < 3:
        # Too few to trim an outlier robustly; fall back to raw min/max.
        lo_s, hi_s = (samples[0], samples[-1]) if samples else (None, None)
    else:
        # Trim the single most-extreme sample on each side.
        lo_s, hi_s = samples[1], samples[-2]
    if lo_s is None or hi_s is None:
        return False
    lo = lo_s / molt.median_s
    hi = hi_s / molt.median_s
    both_win = lo > RED_THRESHOLD and hi > RED_THRESHOLD
    both_lose = lo <= RED_THRESHOLD and hi <= RED_THRESHOLD
    return both_win or both_lose


# ---------------------------------------------------------------------------
# Quiescence guard (#69 Rule 2) — make a bad measurement IMPOSSIBLE
# ---------------------------------------------------------------------------
#
# BEFORE timing, detect contamination and refuse-or-downgrade authority. The
# council's hard finding: the scoreboard picks the WRONG subsystem under load (a
# "0.66 red" was a loaded-machine artifact). This guard is the mechanical kill
# for that class — a board measured on a busy machine is stamped
# ``authoritative=false`` with the EXACT failing check named, and EVERY warm-red
# verdict it produces is EXPLORATORY, never a compiler target.


def _metadata_probe(
    cmd: list[str],
    *,
    timeout_s: float = 10.0,
    cwd: Path | None = None,
) -> subprocess.CompletedProcess[str] | None:
    """Run one bounded host metadata probe.

    This is intentionally the only raw ``subprocess.run`` surface for
    perf-scoreboard host metadata. Workload, build, profiling, and benchmark
    children must use the memory guard; this helper is for tiny read-only host
    probes such as ``sysctl``, ``ps``, ``pgrep``, and ``git``.
    """
    try:
        return subprocess.run(
            cmd,
            cwd=str(cwd) if cwd is not None else None,
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout_s,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None


def _list_build_processes() -> list[dict]:
    """Active cargo/rustc/molt-backend/molt-build processes, EXCLUDING codex.

    Uses ``pgrep -fl`` (full command line). We exclude this very tool's own PID
    and any line containing ``codex`` (project policy: never count/kill codex).
    A pgrep that finds nothing exits 1 with empty stdout — that is the quiet
    case, not an error.
    """
    self_pid = os.getpid()
    parent_pid = os.getppid()
    pattern = "|".join(_BUILD_PROC_PATTERNS)
    res = _metadata_probe(["pgrep", "-fl", pattern], timeout_s=15)
    if res is None:
        # If we cannot probe, we cannot certify quiet — report a sentinel so the
        # caller downgrades authority rather than silently trusting the run.
        return [{"pid": -1, "cmd": "pgrep-unavailable", "probe_failed": True}]
    out: list[dict] = []
    for line in (res.stdout or "").splitlines():
        line = line.strip()
        if not line:
            continue
        head, _, cmd = line.partition(" ")
        if not head.isdigit():
            continue
        pid = int(head)
        cmdl = cmd.lower()
        # Never count codex (project policy) or our own process tree (this tool
        # runs the benchmark builds itself; the pattern would otherwise match a
        # ``molt build`` WE launched — but those run serially BEFORE timing, and
        # the guard is invoked at the start, so excluding our own tree is safe).
        if _CODEX_EXCLUDE in cmdl:
            continue
        if pid in (self_pid, parent_pid):
            continue
        out.append({"pid": pid, "cmd": cmd})
    return out


def _loadavg_1m() -> float | None:
    """1-minute load average via ``sysctl -n vm.loadavg`` (macOS)."""
    res = _metadata_probe(["sysctl", "-n", "vm.loadavg"])
    if res is None:
        return None
    # Output form: "{ 3.28 3.08 3.22 }"
    parts = (res.stdout or "").replace("{", "").replace("}", "").split()
    try:
        return float(parts[0]) if parts else None
    except (ValueError, IndexError):
        return None


def _ncpu() -> int | None:
    res = _metadata_probe(["sysctl", "-n", "hw.ncpu"])
    if res is None:
        return None
    out = (res.stdout or "").strip()
    return int(out) if out.isdigit() else None


def _runnable_thread_count() -> int | None:
    """Count of processes in the RUNNABLE ('R') scheduler state.

    A second contamination signal independent of the 1-minute load EWMA: load
    lags by design (a ~minute time constant), so a build that JUST started reads
    low load but already shows runnable threads. ``ps -A -o stat=`` lists every
    process's state code; a leading 'R' is runnable/running.
    """
    res = _metadata_probe(["ps", "-A", "-o", "stat="])
    if res is None:
        return None
    n = 0
    for line in (res.stdout or "").splitlines():
        if line.strip().startswith("R"):
            n += 1
    return n


def _thermal_ok() -> tuple[bool | None, str | None]:
    """Best-effort thermal/frequency stability via ``pmset -g therm`` (macOS).

    Returns (ok, note). ok=None when pmset is unavailable (skip per the council:
    'best-effort, skip if unavailable'). ok=False iff a thermal/CPU power
    *warning level* is recorded (throttling in progress would skew timings).
    """
    res = _metadata_probe(["pmset", "-g", "therm"])
    if res is None:
        return None, "pmset unavailable"
    text = (res.stdout or "") + (res.stderr or "")
    if not text.strip():
        return None, "pmset returned nothing"
    low = text.lower()
    # macOS prints "No thermal warning level has been recorded" when cool, and a
    # numeric "CPU_Speed_Limit = N" (< 100) when throttled.
    throttled = False
    detail = []
    for line in text.splitlines():
        ls = line.strip()
        if "speed_limit" in ls.lower():
            # e.g. "CPU_Speed_Limit \t = 80"
            digits = "".join(ch for ch in ls.split("=")[-1] if ch.isdigit())
            if digits and int(digits) < 100:
                throttled = True
                detail.append(ls)
        if "thermal warning" in ls.lower() and "no thermal warning" not in ls.lower():
            throttled = True
            detail.append(ls)
    if throttled:
        return False, "; ".join(detail) or "thermal/CPU-speed limit active"
    if "no thermal warning" in low or "no performance warning" in low:
        return True, "no thermal/performance warning recorded"
    return True, "pmset reported no throttle"


def gather_quiescence() -> dict:
    """Measure machine quiescence BEFORE timing (#69 Rule 2).

    Returns a dict with the council-mandated provenance fields plus a ``quiet``
    bool and a ``reasons`` list naming EACH failing check. A run is quiet iff:
      (a) no active cargo/rustc/molt-backend/molt-build process (codex excluded),
      (b) 1-min load average <= ncpu * QUIESCENT_LOAD_FRACTION,
      (c) runnable-thread count does not itself indicate a build storm,
      (d) no thermal/CPU-speed throttle (best-effort; skipped if unavailable).
    This is the contamination detector; the authority decision (refuse vs
    downgrade vs EXPLORATORY) is made by the caller from ``quiet`` + the flags.
    """
    procs = _list_build_processes()
    real_procs = [p for p in procs if not p.get("probe_failed")]
    probe_failed = any(p.get("probe_failed") for p in procs)
    load = _loadavg_1m()
    ncpu = _ncpu()
    runnable = _runnable_thread_count()
    thermal_ok, thermal_note = _thermal_ok()

    reasons: list[str] = []
    if real_procs:
        names = ", ".join(
            f"{p['pid']}:{p['cmd'].split()[0] if p['cmd'] else '?'}"
            for p in real_procs[:6]
        )
        reasons.append(
            f"{len(real_procs)} active build process(es) (cargo/rustc/molt-backend/"
            f"molt build): {names}"
        )
    if probe_failed:
        reasons.append("process probe (pgrep) unavailable — cannot certify quiet")
    load_threshold = (ncpu * QUIESCENT_LOAD_FRACTION) if ncpu else None
    if load is not None and load_threshold is not None and load > load_threshold:
        reasons.append(
            f"1-min load {load:.2f} > threshold {load_threshold:.2f} "
            f"(ncpu={ncpu} * {QUIESCENT_LOAD_FRACTION})"
        )
    elif load is None:
        reasons.append("load average unavailable — cannot certify quiet")
    # Runnable-thread storm: if many threads are runnable the scheduler is
    # contended even if the 1-min EWMA has not caught up. Use the same fraction
    # of ncpu as a sanity ceiling (a quiet desktop shows 0-2 runnable).
    if (
        runnable is not None
        and ncpu is not None
        and runnable > max(2, int(ncpu * QUIESCENT_LOAD_FRACTION))
    ):
        reasons.append(
            f"runnable-thread count {runnable} > {max(2, int(ncpu * QUIESCENT_LOAD_FRACTION))} "
            "(scheduler contended — possible build storm load has not yet caught)"
        )
    if thermal_ok is False:
        reasons.append(f"thermal/CPU-speed throttle active: {thermal_note}")

    quiet = not reasons
    return {
        "quiet": quiet,
        "reasons": reasons,
        # The council-mandated NEW provenance fields:
        "active_molt_processes": [
            p for p in real_procs if "molt" in (p.get("cmd", "").lower())
        ],
        "active_cargo_or_rustc_processes": [
            p
            for p in real_procs
            if "cargo" in p.get("cmd", "").lower()
            or "rustc" in p.get("cmd", "").lower()
        ],
        "active_build_processes": real_procs,
        "loadavg_1m": load,
        "ncpu": ncpu,
        "loadavg_threshold": load_threshold,
        "runnable_signal": runnable,
        "thermal_ok": thermal_ok,
        "thermal_note": thermal_note,
        "probe_failed": probe_failed,
    }


# ---------------------------------------------------------------------------
# Repeat-pass confidence interval (#69 --repeat N) — STABLE iff CI clears 1.00
# ---------------------------------------------------------------------------
#
# A single warm_speedup is a point estimate. The council requires N independent
# measurement PASSES and a CONFIDENCE INTERVAL: a verdict is STABLE only if the
# CI does not straddle 1.00 across passes. This is the mechanical kill for the
# "rediscovered a flaky red" class — a red whose CI crosses 1.00 is a TIE, not a
# target.


def _warm_speedup_ci(
    speedups: list[float],
) -> tuple[float | None, float | None, float | None, float | None]:
    """(median, variance, ci_lo, ci_hi) of per-pass warm_speedups.

    The CI is a Student-t 95% interval on the mean of the passes when n>=2 (the
    small-sample correct choice; z would understate the interval at n=5). With a
    single pass there is no spread to bound — ci is (None, None) and the caller
    must treat the pass as not-yet-confirmed (we never fabricate a tight CI from
    one sample). Variance is the sample variance (n-1).
    """
    vals = [s for s in speedups if isinstance(s, (int, float))]
    if not vals:
        return None, None, None, None
    median = statistics.median(vals)
    if len(vals) < 2:
        return round(median, 4), None, None, None
    mean = statistics.mean(vals)
    var = statistics.variance(vals)
    stdev = var**0.5
    sem = stdev / (len(vals) ** 0.5)
    # 95% two-sided Student-t critical values for small n (df = n-1). Table is
    # exact for the n we use (2..10); beyond that we clamp to the n=10 value
    # (1.833) which is conservative-enough and avoids a scipy dependency.
    t_table = {
        1: 12.706,
        2: 4.303,
        3: 3.182,
        4: 2.776,
        5: 2.571,
        6: 2.447,
        7: 2.365,
        8: 2.306,
        9: 2.262,
    }
    df = len(vals) - 1
    tcrit = t_table.get(df, 2.262 if df >= 9 else 12.706)
    half = tcrit * sem
    return round(median, 4), round(var, 6), round(mean - half, 4), round(mean + half, 4)


def _repeat_stability(ci_lo: float | None, ci_hi: float | None) -> str:
    """Classify the repeat CI vs the 1.00 floor.

    Returns one of: 'STABLE_BELOW' (CI entirely < 1.0 — a real red),
    'STABLE_ABOVE' (CI entirely > 1.0 — a real green), 'STRADDLES' (CI crosses
    1.0 — a TIE), or 'UNCONFIRMED' (a single pass; no CI to judge).
    """
    if ci_lo is None or ci_hi is None:
        return "UNCONFIRMED"
    if ci_hi < RED_THRESHOLD:
        return "STABLE_BELOW"
    if ci_lo > RED_THRESHOLD:
        return "STABLE_ABOVE"
    return "STRADDLES"


# ---------------------------------------------------------------------------
# Cycle attribution (#69 Rule 1 + --emit-cycle-profile) — CYCLES, not allocs
# ---------------------------------------------------------------------------
#
# Rule 1: alloc-attribution alone CANNOT justify a warm-time opt. A warm red
# needs CYCLE attribution. For each warm red we capture a CYCLE profile (macOS
# ``/usr/bin/sample``) of the running molt binary and attach the top symbols.
# This is the signal the NEXT optimization is steered by — never the alloc count.


def _resolve_sampler() -> str | None:
    """Path to the macOS ``sample`` CPU profiler, or None if unavailable."""
    import shutil

    for cand in ("/usr/bin/sample", shutil.which("sample") or ""):
        if cand and Path(cand).exists():
            return cand
    return None


def _profiling_popen(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
) -> subprocess.Popen[str]:
    """Start one interactive profiling child under MOLT_BENCH process custody."""
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH", env)
    kwargs = harness_memory_guard.batch_process_group_kwargs(limits, env=env)
    return subprocess.Popen(
        cmd,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
        **kwargs,
    )


def capture_cycle_profile(
    cmd: list[str],
    *,
    env: dict[str, str],
    rss_mb: int,
    timeout_s: float,
    sample_seconds: int = 3,
    top_n: int = 25,
    repeat_runs: int = 40,
) -> dict:
    """Run ``cmd`` under safe_run and CPU-sample it with ``/usr/bin/sample``.

    Two robustness measures defeat the attach race that loses short benchmarks:

      1. ``sample -wait <name>`` is started FIRST and BLOCKS until a process of
         that name launches, then samples it from t=0 — no poll-for-PID race
         (the prior approach lost any benchmark that exited before the poll).
      2. The molt binary is run ``repeat_runs`` times BACK-TO-BACK in one process
         group under safe_run, so even a 30 ms benchmark presents a multi-second
         CPU window for the 1 ms-interval sampler to accumulate a stable
         self-time leaderboard. The benchmark's own work is unchanged — this is a
         longer PROFILING run of the same code, not a different workload.

    Returns ``{available, top_symbols, note}``. If the sampler is unavailable or
    the process never presents enough CPU to sample, returns a documented
    ``available=False`` note — never a fabricated signal (Rule 1).
    """
    sampler = _resolve_sampler()
    if sampler is None:
        return {
            "available": False,
            "top_symbols": [],
            "note": "cycle profiler unavailable (/usr/bin/sample not found)",
        }
    import tempfile

    out_file = Path(
        tempfile.mktemp(prefix="perfscore-sample-", suffix=".txt", dir="/tmp")
    )
    target_name = Path(cmd[0]).name
    # Run the binary repeat_runs times back-to-back inside ONE safe_run-guarded
    # process group so the sampler has a multi-second window. A non-zero exit on
    # any iteration stops the loop (set -e) — we still sample whatever ran.
    quoted = " ".join(_shquote(a) for a in cmd)
    loop_cmd = (
        f"for i in $(seq 1 {repeat_runs}); do {quoted} >/dev/null 2>&1 || break; done"
    )
    safe_cmd = [
        sys.executable,
        str(SAFE_RUN),
        "--rss-mb",
        str(rss_mb),
        "--timeout",
        str(max(timeout_s, sample_seconds + 10)),
        "--",
        "/bin/sh",
        "-c",
        loop_cmd,
    ]
    # Start the sampler FIRST in -wait mode (race-free): it blocks until a process
    # named target_name appears, then samples it. We launch it as a background
    # Popen, then start the workload; sample catches the workload at launch.
    try:
        sampler_proc = _profiling_popen(
            [
                sampler,
                target_name,
                str(sample_seconds),
                "-wait",
                "-mayDie",
                "-f",
                str(out_file),
            ]
        )
    except OSError as exc:
        return {
            "available": False,
            "top_symbols": [],
            "note": f"could not start sampler in -wait mode: {exc!r}",
        }
    try:
        proc = _profiling_popen(safe_cmd, env=env)
    except OSError as exc:
        _terminate(sampler_proc)
        return {
            "available": False,
            "top_symbols": [],
            "note": f"could not launch target for profiling: {exc!r}",
        }
    # Let the sampler run its window, then reap both. The sampler exits on its own
    # after sample_seconds (or when the target dies under -mayDie); the workload
    # loop exits when its runs complete or safe_run caps it.
    try:
        sampler_proc.wait(timeout=sample_seconds + 30)
    except subprocess.TimeoutExpired:
        _terminate(sampler_proc)
    srv_rc = sampler_proc.returncode
    _terminate(proc)
    symbols = _parse_sample_heaviest(out_file, top_n=top_n)
    try:
        out_file.unlink()
    except OSError:
        pass
    if not symbols:
        return {
            "available": False,
            "top_symbols": [],
            "note": (
                f"sampler produced no parseable symbols (rc={srv_rc}); benchmark "
                "may present too little CPU even across "
                f"{repeat_runs} back-to-back runs — cycle attribution unavailable"
            ),
        }
    return {
        "available": True,
        "top_symbols": symbols,
        "note": (
            f"/usr/bin/sample {sample_seconds}s self-time over {repeat_runs} "
            "back-to-back runs (CYCLES, not alloc-count)"
        ),
    }


def _shquote(arg: str) -> str:
    """Minimal POSIX shell quote for embedding an argv element in `sh -c`."""
    import shlex

    return shlex.quote(arg)


def _terminate(proc: subprocess.Popen) -> None:
    harness_memory_guard.force_close_process_group(proc)


def _parse_sample_heaviest(out_file: Path, *, top_n: int) -> list[dict]:
    """Parse ``/usr/bin/sample``'s output for the heaviest self-time symbols.

    ``sample`` emits a 'Sort by top of stack' section listing
    ``<count>  <symbol>  (in <lib>)`` lines — the self-time leaders. We read that
    section (the cycle-attribution signal) and return the top ``top_n`` as
    ``{symbol, self_samples, lib}``.
    """
    try:
        text = out_file.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    lines = text.splitlines()
    # Locate the self-time leaderboard.
    start = None
    for i, line in enumerate(lines):
        if "Sort by top of stack" in line:
            start = i + 1
            break
    if start is None:
        return []
    out: list[dict] = []
    for line in lines[start:]:
        s = line.strip()
        if not s:
            if out:
                break
            continue
        if s.startswith("Binary Images"):
            break
        # macOS `sample` self-time leaderboard form (count is the TRAILING token):
        #   "<symbol>  (in <lib>)        <count>"
        #   "<symbol>        <count>"            (no lib)
        # Split the trailing integer off the end; everything before is the
        # symbol (+ optional "(in lib)").
        toks = s.rsplit(None, 1)
        if len(toks) != 2 or not toks[1].isdigit():
            continue
        count = int(toks[1])
        rest = toks[0].strip()
        lib = None
        if "(in " in rest:
            sym, _, tail = rest.partition("(in ")
            lib = tail.split(")")[0].strip()
            symbol = sym.strip()
        else:
            symbol = rest
        out.append({"symbol": symbol, "self_samples": count, "lib": lib})
        if len(out) >= top_n:
            break
    return out


# ---------------------------------------------------------------------------
# Warm-hot cycle attribution (#76): inner-repeat + symbolicate + hot-only sample
# ---------------------------------------------------------------------------


def _is_launch_frame(symbol: str, lib: str | None) -> bool:
    """True iff a leaf frame is process launch / first-touch page-in, not work.

    ``_dyld_start`` (in dyld) is the dynamic-loader entry: it covers process
    launch AND the first-touch page-in of the static binary's text. That is the
    cost inner-repeat exists to amortize; if it still dominates, the loop factor
    was too small (the refusal gate fires).
    """
    sym = (symbol or "").lstrip("_")
    for ls, ll in _LAUNCH_FRAMES:
        if sym == ls.lstrip("_") and (lib or "") == ll:
            return True
    return False


def classify_launch_dominance(top_symbols: list[dict]) -> dict:
    """Compute the launch/page-in vs in-binary breakdown of a sample leaderboard.

    Returns ``{total, launch_samples, launch_fraction, in_binary_samples,
    in_binary_fraction, launch_dominates}``. ``launch_dominates`` is the
    refusal signal: True iff launch/page-in is >= the refusal fraction of the
    whole leaf-self-time leaderboard, meaning the steady-state hot path is NOT
    yet legible and a hot-path claim must be refused.
    """
    total = sum(int(s.get("self_samples", 0)) for s in top_symbols)
    if total <= 0:
        return {
            "total": 0,
            "launch_samples": 0,
            "launch_fraction": None,
            "in_binary_samples": 0,
            "in_binary_fraction": None,
            "launch_dominates": True,  # no signal -> cannot attribute -> refuse
        }
    launch = sum(
        int(s.get("self_samples", 0))
        for s in top_symbols
        if _is_launch_frame(s.get("symbol", ""), s.get("lib"))
    )
    launch_frac = launch / total
    return {
        "total": total,
        "launch_samples": launch,
        "launch_fraction": round(launch_frac, 4),
        "in_binary_samples": total - launch,
        "in_binary_fraction": round((total - launch) / total, 4),
        "launch_dominates": launch_frac >= LAUNCH_DOMINANCE_REFUSAL_FRACTION,
    }


def top_in_binary_frames(
    top_symbols: list[dict], *, binary_lib: str | None, top_n: int = 20
) -> list[dict]:
    """The heaviest IN-BINARY (molt user/runtime) frames — the cycle facts.

    Filters the leaderboard to frames whose ``lib`` is the profiled binary (so
    libsystem/dyld helpers are excluded) and annotates each with its share of
    the WHOLE leaderboard (``leaderboard_pct``), which is the attribution unit.
    """
    total = sum(int(s.get("self_samples", 0)) for s in top_symbols) or 1
    out: list[dict] = []
    for s in top_symbols:
        lib = s.get("lib")
        if binary_lib is not None and lib != binary_lib:
            continue
        if s.get("symbol") == "???":
            # An unsymbolicated in-binary frame: record it (with its offset text
            # if the parser preserved it) but it is not yet a named cycle fact.
            pass
        out.append(
            {
                "symbol": s.get("symbol"),
                "self_samples": int(s.get("self_samples", 0)),
                "leaderboard_pct": round(
                    100.0 * int(s.get("self_samples", 0)) / total, 2
                ),
                "lib": lib,
            }
        )
        if len(out) >= top_n:
            break
    return out


def build_profiling_binary(
    script_path: Path,
    *,
    spec: "BackendSpec",
    profile: str,
    inner_loops: int,
    log_lines: list[str],
) -> "tuple[bench.MoltBinary | None, dict]":
    """Build the LOOPED + SYMBOLICATED profiling variant of a benchmark.

    Two transforms vs the normal cell build:
      1. INNER-REPEAT — the benchmark's ``main()`` is wrapped in
         ``for _ in range(N): main()`` (``perf_inner_repeat``) so launch/page-in
         amortizes inside one process. Refuses (and returns the reason) if the
         benchmark is not the semantics-preservingly loopable shape.
      2. SYMBOLICATE — built with ``MOLT_KEEP_SYMBOLS=1`` so the final link
         keeps molt user-fn / runtime symbol names.

    Returns ``(binary_or_None, meta)`` where ``meta`` documents the transform
    (``inner_loops``, ``symbolicated``, ``looped``, ``refused``/``reason``).
    This binary is for CYCLE ATTRIBUTION ONLY — never for the speedup number
    (the timing path measures the shipped, stripped one-shot binary).
    """
    meta: dict = {
        "inner_loops": inner_loops,
        "symbolicated": True,
        "looped": False,
        "refused": False,
        "reason": None,
        "looped_source_path": None,
    }
    try:
        source = script_path.read_text(encoding="utf-8")
    except OSError as exc:
        meta["refused"] = True
        meta["reason"] = f"could not read benchmark source: {exc!r}"
        return None, meta

    plan = perf_inner_repeat.analyze(source, inner_loops=inner_loops)
    if not plan.ok:
        meta["refused"] = True
        meta["reason"] = f"inner-repeat refused: {plan.reason}"
        log_lines.append(f"PROFILING-BUILD REFUSED (inner-repeat): {plan.reason}")
        return None, meta
    meta["looped"] = True

    # Write the looped variant next to a temp dir; the build reads it as a normal
    # script. The name carries the benchmark stem so the in-binary lib name (the
    # sample 'in <lib>') is recognizable.
    looped_dir = Path(
        tempfile.mkdtemp(prefix="perfscore-loop-", dir=str(_profiling_tmp_root()))
    )
    looped_path = looped_dir / script_path.name
    looped_path.write_text(plan.source, encoding="utf-8")
    meta["looped_source_path"] = str(looped_path)

    build_env = _perfscore_build_env(spec)
    build_env[MOLT_KEEP_SYMBOLS_ENV] = "1"  # the symbolication hatch
    extra_args = bench_suites.molt_args_for_benchmark(script_path)
    build_flag = PROFILE_BUILD_FLAG.get(profile, "release")
    try:
        binary = bench.prepare_molt_binary(
            str(looped_path),
            extra_args=extra_args,
            env=build_env,
            build_profile=build_flag,
            batch_server=None,  # symbolicated env differs from the cell server's
            build_timeout_s=600.0,
        )
    except Exception as exc:  # noqa: BLE001 - record, never crash the sweep
        meta["refused"] = True
        meta["reason"] = f"profiling build raised: {exc!r}"
        log_lines.append(f"PROFILING-BUILD EXCEPTION: {exc!r}")
        return None, meta
    if not isinstance(binary, bench.MoltBinary):
        meta["refused"] = True
        if isinstance(binary, bench.MoltFailure):
            meta["reason"] = f"profiling build failed: {binary.status}"
            detail = f" detail={binary.detail}" if binary.detail else ""
            log_lines.append(f"PROFILING-BUILD FAILED status={binary.status}{detail}")
        else:
            meta["reason"] = "profiling build produced no binary"
            log_lines.append("PROFILING-BUILD FAILED")
        return None, meta
    log_lines.append(
        f"profiling binary built: looped(inner_loops={inner_loops}) + symbolicated "
        f"size_kib={round(binary.size_kb, 1)}"
    )
    return binary, meta


def _time_one_run(
    cmd: list[str], *, env: dict[str, str], rss_mb: int, timeout_s: float
) -> "RunOutcome":
    """Wall-time ONE run of ``cmd`` under safe_run (for sizing the sample window).

    Returns the full RunOutcome so the caller can distinguish an OOM (an
    inner-repeat that amplifies a per-iteration leak past the RSS cap — a real
    finding) from a generic run failure.
    """
    return _safe_run_json(
        cmd,
        env=env,
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label="hot-size",
    )


def capture_hot_only_profile(
    binary_path: Path,
    *,
    run_args: list[str],
    env: dict[str, str],
    rss_mb: int,
    inner_loops: int,
    warmup_s: float = HOT_SAMPLE_WARMUP_S,
    window_s: float = HOT_SAMPLE_WINDOW_S,
    top_n: int = 30,
) -> dict:
    """Sample a LOOPED+SYMBOLICATED process in STEADY STATE (#76).

    Three steps, all in ONE process per phase so launch/page-in is paid once and
    the steady-state hot path dominates:

      1. SIZE — time one looped run to learn its lifetime ``T``. The inner-repeat
         must keep the process alive for at least ``warmup_s + a short window``;
         if ``T`` is too short, the loop factor is too small to carve out a
         steady-state window and we REFUSE ("increase --inner-repeat") — never
         sample a process that exits mid-window.
      2. WARMUP — launch the looped process, sleep ``warmup_s`` so the first
         iterations (cold I-cache, first-touch page-in) are NOT in the window,
         then attach ``/usr/bin/sample`` to the already-running process (no
         ``-wait`` needed; ``/usr/bin/sample`` has no built-in warmup delay, so
         the warmup is realized by delaying the attach).
      3. SAMPLE — accumulate leaf self-time for ``window`` seconds of steady
         state, fitted to the remaining lifetime so it closes before exit.

    Applies the REFUSAL rule: if launch/page-in still accounts for >= the
    refusal fraction of leaf self-time, the loop factor was too small — returns
    ``available=False`` with ``refused_reason`` and NO hot-path claim (fail
    closed, same as #69's quiescence guard). Otherwise returns the in-binary hot
    frames (the cycle facts that select the next optimization).
    """
    import time as _time

    sampler = _resolve_sampler()
    if sampler is None:
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": [],
            "in_binary_top": [],
            "launch_breakdown": None,
            "refused": True,
            "refused_reason": "cycle profiler unavailable (/usr/bin/sample not found)",
            "note": "sampler unavailable",
        }
    target_name = binary_path.name
    cmd = [str(binary_path), *run_args]

    # --- (1) SIZE: time one looped run so we can fit the steady-state window ---
    size = _time_one_run(
        cmd, env=env, rss_mb=rss_mb, timeout_s=warmup_s + window_s + 120
    )
    if not size.ok:
        if size.status == "oom":
            reason = (
                f"looped(inner_loops={inner_loops}) binary exceeded the {rss_mb} MiB "
                "RSS cap — the inner-repeat amplified a per-iteration molt LEAK "
                "(each main() call leaks its working set; a one-shot run hides it). "
                "LOWER --inner-repeat to profile a bounded window; the leak itself "
                "is a separate compiler-RC finding"
            )
            note = "size run OOM (inner-repeat amplified a per-iteration leak)"
        else:
            reason = (
                f"looped profiling binary failed to run (size phase: {size.status})"
            )
            note = "size run failed"
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": [],
            "in_binary_top": [],
            "launch_breakdown": None,
            "leak_suspected": size.status == "oom",
            "refused": True,
            "refused_reason": reason,
            "note": note,
            "size_status": size.status,
            "size_exit_code": size.exit_code,
            "size_stdout_tail": size.stdout_tail,
            "size_stderr_tail": size.stderr_tail,
        }
    looped_runtime_s = size.elapsed_s or 0.0
    # The steady window we can actually carve out after warmup, leaving a 0.3s
    # tail so the sampler closes before the process exits. Need a real window.
    min_window_s = 0.8
    avail_window_s = looped_runtime_s - warmup_s - 0.3
    if avail_window_s < min_window_s:
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": [],
            "in_binary_top": [],
            "launch_breakdown": None,
            "refused": True,
            "refused_reason": (
                f"CYCLE-ATTRIBUTION INVALID: looped runtime {looped_runtime_s:.2f}s "
                f"is too short to carve a steady-state window after {warmup_s:.1f}s "
                f"warmup (need >= {warmup_s + 0.3 + min_window_s:.1f}s) — "
                "increase --inner-repeat"
            ),
            "note": "looped runtime too short for a steady window",
        }
    eff_window_s = max(min_window_s, min(window_s, avail_window_s))

    out_file = Path(tempfile.mktemp(prefix="perfscore-hot-", suffix=".txt", dir="/tmp"))
    quoted = " ".join(_shquote(a) for a in cmd)
    run_one = f"{quoted} >/dev/null 2>&1 || true"
    safe_cmd = [
        sys.executable,
        str(SAFE_RUN),
        "--rss-mb",
        str(rss_mb),
        "--timeout",
        str(int(warmup_s + eff_window_s + 60)),
        "--",
        "/bin/sh",
        "-c",
        run_one,
    ]
    # --- (2) WARMUP: launch the workload, sleep warmup_s, THEN attach ---------
    try:
        proc = _profiling_popen(safe_cmd, env=env)
    except OSError as exc:
        try:
            out_file.unlink()
        except OSError:
            pass
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": [],
            "in_binary_top": [],
            "launch_breakdown": None,
            "refused": True,
            "refused_reason": f"could not launch profiling target: {exc!r}",
            "note": "target launch failed",
        }
    _time.sleep(warmup_s)  # let the first iterations warm up (excluded from window)
    # --- (3) SAMPLE: attach to the now-running steady-state process -----------
    try:
        sampler_proc = _profiling_popen(
            [
                sampler,
                target_name,
                str(max(1, int(round(eff_window_s)))),
                "-mayDie",
                "-f",
                str(out_file),
            ]
        )
    except OSError as exc:
        _terminate(proc)
        try:
            out_file.unlink()
        except OSError:
            pass
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": [],
            "in_binary_top": [],
            "launch_breakdown": None,
            "refused": True,
            "refused_reason": f"could not start sampler: {exc!r}",
            "note": "sampler start failed",
        }
    try:
        sampler_proc.wait(timeout=eff_window_s + 40)
    except subprocess.TimeoutExpired:
        _terminate(sampler_proc)
    _terminate(proc)
    symbols = _parse_sample_heaviest(out_file, top_n=max(top_n, 60))
    try:
        out_file.unlink()
    except OSError:
        pass
    if not symbols:
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": [],
            "in_binary_top": [],
            "launch_breakdown": None,
            "refused": True,
            "refused_reason": (
                "sampler produced no parseable symbols even after fitting the "
                "window to the looped runtime — raise --inner-repeat so the "
                "steady-state window is longer"
            ),
            "note": "no samples",
        }
    breakdown = classify_launch_dominance(symbols)
    note = (
        f"/usr/bin/sample {eff_window_s:.1f}s steady-state (after {warmup_s:.1f}s "
        f"warmup) of ONE looped(inner_loops={inner_loops}) + symbolicated process; "
        f"looped runtime {looped_runtime_s:.2f}s — CYCLES"
    )
    if breakdown["launch_dominates"]:
        lf = breakdown["launch_fraction"]
        lf_pct = f"{100 * lf:.1f}%" if lf is not None else "n/a"
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": symbols[:top_n],
            "in_binary_top": [],
            "launch_breakdown": breakdown,
            "refused": True,
            "refused_reason": (
                f"CYCLE-ATTRIBUTION INVALID: launch/page-in dominates leaf "
                f"self-time ({lf_pct} >= "
                f"{int(100 * LAUNCH_DOMINANCE_REFUSAL_FRACTION)}%) even after "
                f"inner-repeat — increase --inner-repeat"
            ),
            "note": note,
        }
    in_binary_top = top_in_binary_frames(symbols, binary_lib=target_name, top_n=top_n)
    return {
        "available": True,
        "mode": "hot-only",
        "inner_loops": inner_loops,
        "top_symbols": symbols[:top_n],
        "in_binary_top": in_binary_top,
        "launch_breakdown": breakdown,
        "refused": False,
        "refused_reason": None,
        "note": note,
    }


def _profiling_tmp_root() -> Path:
    """Temp root for looped profiling sources/binaries (created on demand)."""
    root = Path(tempfile.gettempdir()) / "perfscore_profiling"
    root.mkdir(parents=True, exist_ok=True)
    return root


def run_hot_only_profiles(
    *,
    scripts: list[Path],
    spec: "BackendSpec",
    profile: str,
    inner_loops: int,
    rss_mb: int,
    warmup_s: float = HOT_SAMPLE_WARMUP_S,
    window_s: float = HOT_SAMPLE_WINDOW_S,
    top_n: int = 30,
) -> list[dict]:
    """Drive the #76 hot-only profiler for each benchmark and return per-bench cells.

    For each benchmark: build the LOOPED + SYMBOLICATED variant, sample its
    steady state, apply the refusal gate, and collect the in-binary hot frames.
    Each returned cell is a self-contained attribution record (the cycle fact),
    NOT a scoreboard speedup cell — these never touch the gate.
    """
    results: list[dict] = []
    for script in scripts:
        key = bench_suites.canonical_benchmark_key(script)
        log_lines: list[str] = [f"# HOT-ONLY {key} | {spec.backend} | {profile}"]
        print(
            f"[hot-only] {key} | inner_loops={inner_loops} | symbolicated ...",
            file=sys.stderr,
            flush=True,
        )
        binary, build_meta = build_profiling_binary(
            script,
            spec=spec,
            profile=profile,
            inner_loops=inner_loops,
            log_lines=log_lines,
        )
        cell: dict = {
            "benchmark": key,
            "target": spec.target,
            "backend": spec.backend,
            "profile": profile,
            "inner_loops": inner_loops,
            "build": build_meta,
        }
        if binary is None:
            cell["profile_result"] = {
                "available": False,
                "refused": True,
                "refused_reason": build_meta.get("reason"),
            }
            results.append(cell)
            print(
                f"    -> REFUSED ({build_meta.get('reason')})",
                file=sys.stderr,
                flush=True,
            )
            continue
        try:
            run_args = bench.resolve_benchmark_run_args(str(script))
            prof = capture_hot_only_profile(
                Path(binary.path),
                run_args=run_args,
                env=_perfscore_build_env(spec),
                rss_mb=rss_mb,
                inner_loops=inner_loops,
                warmup_s=warmup_s,
                window_s=window_s,
                top_n=top_n,
            )
        finally:
            _release_binary(binary)
        cell["profile_result"] = prof
        results.append(cell)
        if prof.get("refused"):
            print(
                f"    -> REFUSED ({prof.get('refused_reason')})",
                file=sys.stderr,
                flush=True,
            )
        else:
            bd = prof.get("launch_breakdown") or {}
            lf = bd.get("launch_fraction")
            top = prof.get("in_binary_top") or []
            head = top[0]["symbol"] if top else "-"
            print(
                f"    -> HOT (launch={100 * lf:.1f}% < "
                f"{int(100 * LAUNCH_DOMINANCE_REFUSAL_FRACTION)}%) "
                f"top: {head}",
                file=sys.stderr,
                flush=True,
            )
    return results


def _emit_hot_only_board(
    hot_cells: list[dict],
    *,
    spec: "BackendSpec",
    profile: str,
    inner_loops: int,
    quiescence: dict,
    cpython_version: str,
    out: str | None,
) -> int:
    """Write the #76 hot-only profile board (JSON) + print the attribution report.

    Returns 0 when every requested benchmark produced a hot-path attribution,
    1 when ANY was REFUSED (launch still dominated, or the build/transform was
    refused) — the fail-closed signal that the loop factor or shape needs work.
    """
    git_rev = _git_rev()
    SCOREBOARD_DIR.mkdir(parents=True, exist_ok=True)
    out_path = (
        Path(out)
        if out
        else SCOREBOARD_DIR / f"hot_profile_{spec.backend}_{git_rev}.json"
    )
    doc = {
        "schema_version": SCHEMA_VERSION,
        "kind": "hot_only_cycle_profile",
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": git_rev,
        "backend": spec.backend,
        "target": spec.target,
        "profile": profile,
        "inner_loops": inner_loops,
        "symbolicated": True,
        "symbolicate_mechanism": f"{MOLT_KEEP_SYMBOLS_ENV}=1 (link-strip + post-link strip skipped)",
        "launch_refusal_fraction": LAUNCH_DOMINANCE_REFUSAL_FRACTION,
        "cpython_baseline": cpython_version,
        "quiescence": quiescence,
        "methodology": (
            "Inner-repeat the benchmark main() N times in ONE process so launch/"
            "page-in (_dyld_start) amortizes; build with MOLT_KEEP_SYMBOLS=1 so the "
            "linker keeps molt user-fn/runtime symbols; /usr/bin/sample the steady "
            "state after a warmup delay. REFUSE a hot-path claim if launch/page-in "
            f">= {int(100 * LAUNCH_DOMINANCE_REFUSAL_FRACTION)}% of leaf self-time."
        ),
        "cells": hot_cells,
    }
    out_path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")

    # --- Report ----------------------------------------------------------
    print("\n" + "=" * 72)
    print(f"WARM-HOT CYCLE ATTRIBUTION (#76) — {spec.backend}/{profile}")
    print(f"  inner_loops={inner_loops}  symbolicated={MOLT_KEEP_SYMBOLS_ENV}=1")
    print("=" * 72)
    any_refused = False
    for cell in hot_cells:
        pr = cell.get("profile_result", {})
        key = cell["benchmark"]
        if pr.get("refused") or not pr.get("available"):
            any_refused = True
            print(f"\n  {key}")
            print(f"    REFUSED: {pr.get('refused_reason')}")
            bd = pr.get("launch_breakdown")
            if bd and bd.get("launch_fraction") is not None:
                print(
                    f"    (launch/page-in = {100 * bd['launch_fraction']:.1f}% of "
                    f"{bd['total']} leaf samples)"
                )
            continue
        bd = pr.get("launch_breakdown", {})
        lf = bd.get("launch_fraction")
        ibf = bd.get("in_binary_fraction")
        print(f"\n  {key}  [HOT — attribution VALID]")
        print(
            f"    launch/page-in: {100 * lf:.1f}%   in-binary: {100 * ibf:.1f}%   "
            f"({bd['total']} leaf samples)"
        )
        print("    TOP IN-BINARY HOT FRAMES (the cycle facts):")
        for s in (pr.get("in_binary_top") or [])[:12]:
            print(f"      {s['leaderboard_pct']:5.1f}%  {s['symbol']}")
    try:
        shown = out_path.relative_to(REPO_ROOT)
    except ValueError:
        shown = out_path  # --out outside the repo (e.g. /tmp): show absolute
    print(f"\n  -> wrote {shown}")
    if any_refused:
        print(
            "  -> SOME benchmarks REFUSED (launch dominated / not loopable): "
            "raise --inner-repeat or inspect the reason.",
        )
        return 1
    print("  -> all benchmarks attributed to in-binary hot frames.")
    return 0


# ---------------------------------------------------------------------------
# One scoreboard cell: benchmark x target x backend x profile
# ---------------------------------------------------------------------------


@dataclass
class Cell:
    benchmark: str
    target: str
    backend: str
    profile: str

    # Build facts (from the daemon batch build).
    build_ok: bool = False
    binary_size_kib: float | None = None
    compile_time_s: float | None = None

    # Run facts.
    run_blocked: bool = False
    run_blocked_reason: str | None = None
    molt_ok: bool = False
    cpython_ok: bool = False

    # COLD: first cold-cache sample for each runtime.
    cold_molt_s: float | None = None
    cold_cpython_s: float | None = None

    # WARM: steady-state median for each runtime.
    warm_molt_s: float | None = None
    warm_cpython_s: float | None = None

    # Peak RSS (the worse of cold/warm samples) for each runtime.
    molt_peak_rss_mib: float | None = None
    cpython_peak_rss_mib: float | None = None

    # Stability + diagnostic note.
    stable: bool = False
    note: str | None = None

    # --- Two-dimensional verdict (council ruling A) ----------------------
    # warm_speedup = cpython_warm / molt_warm  (the EXECUTION-ENGINE axis).
    # cold_speedup = cpython_cold / molt_cold  (the END-TO-END cold axis).
    # startup_tax_ms = (molt_cold_total - molt_warm_total) * 1000  (fixed tax).
    warm_speedup: float | None = None
    cold_speedup: float | None = None
    startup_tax_ms: float | None = None
    cold_budget_ms: float | None = None  # the budget this cell was gated against
    verdict: str = "pending"  # one of the VERDICT_* constants
    # The single most-likely missing fact / startup component, for triage.
    suspected_missing_fact: str | None = None
    suspected_startup_component: str | None = None

    # --- PyPy / Codon comparator lanes (council Lane C; nullable) ---------
    # pypy_ratio = pypy_warm / molt_warm  (>1 = molt faster than PyPy).
    # codon_ratio = codon_warm / molt_warm (>1 = molt faster than Codon).
    pypy_ratio: float | None = None
    pypy_warm_s: float | None = None
    codon_ratio: float | None = None
    codon_warm_s: float | None = None
    # Codon is AOT but NOT drop-in for every benchmark; a non-equivalent
    # benchmark is NEVER scored win/loss — only recorded as "non-equivalent".
    codon_equivalent: bool | None = None
    codon_note: str | None = None

    # When the CPython BASELINE itself cannot run the script (e.g. a
    # molt-internal benchmark that imports molt-only modules, or needs args the
    # bare CPython invocation lacks), there is no valid floor to compare
    # against. Such a cell is NOT red — it is excluded from the gate as
    # cpython-incompatible (a benchmark-harness fact, not a molt slowness fact).
    cpython_incompatible: bool = False

    # Provenance.
    output_parity: bool | None = None
    molt_stats: dict | None = None
    cpython_stats: dict | None = None
    log_artifact: str | None = None

    # --- Measurement-hygiene classification (#69 --classify) --------------
    # The council's 5-state classification (orthogonal to ``verdict``): the
    # answer to "TRUE compiler target or measurement artifact?". Set only when
    # --classify is requested; None otherwise so the 2-D verdict path is
    # unchanged for callers that do not opt in.
    classification: str | None = None  # one of CLASSIFY_STATES
    classification_reason: str | None = None
    measured_quiescent: bool | None = None

    # --- Repeat-pass CI (#69 --repeat N) ----------------------------------
    # Per-pass warm_speedups + the Student-t 95% CI; a verdict is STABLE only if
    # the CI does not straddle 1.00. Empty/None when --repeat is not used.
    repeat_passes: int | None = None
    repeat_warm_speedups: list[float] | None = None
    repeat_median_warm: float | None = None
    repeat_variance: float | None = None
    repeat_ci_lo: float | None = None
    repeat_ci_hi: float | None = None
    repeat_stability: str | None = (
        None  # STABLE_BELOW|STABLE_ABOVE|STRADDLES|UNCONFIRMED
    )

    # --- Cycle attribution (#69 Rule 1 + --emit-cycle-profile) ------------
    # The CYCLE profile (NOT alloc-count) for warm reds — the next-opt signal.
    cycle_profile: dict | None = None

    def finalize(
        self,
        *,
        budget_ms: float | None = None,
        authoritative: bool = True,
    ) -> None:
        """Derive speedups + the TWO-DIMENSIONAL verdict from collected facts.

        ``budget_ms`` is the cold-start tax budget for this (backend, profile)
        cell (None = no budget recorded yet; FAIL_COLD_BUDGET cannot fire).
        ``authoritative`` False stamps every cell FAIL_STALE (the tree is not
        origin/main) — overriding all other verdicts per council ruling A.

        Sets ``verdict`` (the VERDICT_* vocabulary), the single gate authority
        consumed by summaries, rebuilds, merges, and CI.
        """
        self.cold_budget_ms = budget_ms

        # FAIL_STALE overrides everything: a non-authoritative tree's numbers
        # are not the origin/main contract, full stop.
        if not authoritative:
            self.verdict = VERDICT_FAIL_STALE
            if self.note is None:
                self.note = "non-authoritative tree (local != origin/main or dirty)"
            return

        if self.run_blocked:
            self.verdict = VERDICT_RUN_BLOCKED
            return
        if not self.build_ok:
            self.verdict = VERDICT_BUILD_FAILED
            return
        # CPython baseline can't run -> no valid floor; not gated.
        if not self.cpython_ok:
            self.cpython_incompatible = True
            self.verdict = VERDICT_CPY_INCOMPAT
            if self.note is None:
                self.note = "CPython baseline could not run this script standalone"
            return
        # CPython runs but molt does not -> a real molt run failure.
        if not self.molt_ok:
            self.verdict = VERDICT_RUN_ERROR
            if self.note is None:
                self.note = "molt run failed/unmeasurable while CPython ran"
            return

        self.warm_speedup = _safe_ratio(self.warm_cpython_s, self.warm_molt_s)
        self.cold_speedup = _safe_ratio(self.cold_cpython_s, self.cold_molt_s)

        # startup_tax_ms = the fixed cold-start cost molt pays that the warm
        # steady state does not (cold_total - warm_total). This is the quantity
        # the cold-start budget gates against — NOT the cold/cpython ratio.
        if self.cold_molt_s is not None and self.warm_molt_s is not None:
            self.startup_tax_ms = round(
                (self.cold_molt_s - self.warm_molt_s) * 1000.0, 2
            )

        # Unstable cell: cannot be trusted in EITHER direction -> gated.
        if not self.stable:
            self.verdict = VERDICT_UNSTABLE
            return

        warm_below = (
            self.warm_speedup is not None and self.warm_speedup <= RED_THRESHOLD
        )
        cold_below = (
            self.cold_speedup is not None and self.cold_speedup <= RED_THRESHOLD
        )
        over_budget = (
            budget_ms is not None
            and self.startup_tax_ms is not None
            and self.startup_tax_ms > budget_ms
        )

        # --- The two-dimensional decision (council ruling A) -------------
        # 1. FAIL_ENGINE — warm steady-state is at/below CPython. This is the
        #    execution-engine red, the release blocker. It dominates a cold
        #    failure (if the engine is slow, fix the engine first).
        if warm_below:
            self.verdict = VERDICT_FAIL_ENGINE
            self.suspected_missing_fact = self.suspected_missing_fact or _suspect_fact(
                self.benchmark
            )
            return
        # 2. FAIL_COLD_BUDGET — warm is fine but the fixed startup tax exceeds
        #    the recorded budget for this lane. A startup regression, not an
        #    engine red; routes to the cold-start lane.
        if over_budget:
            self.verdict = VERDICT_FAIL_COLD_BUDGET
            self.suspected_startup_component = (
                self.suspected_startup_component
                or _suspect_startup_component(self.benchmark)
            )
            return
        # 3. WARN_COLD_FLOOR — warm > CPython, but cold <= CPython AND the loss
        #    is solely the fixed startup tax (within budget). NOT an engine red;
        #    does not fail the gate unless --strict-cold.
        if cold_below:
            self.verdict = VERDICT_WARN_COLD_FLOOR
            self.suspected_startup_component = (
                self.suspected_startup_component
                or _suspect_startup_component(self.benchmark)
            )
            return
        # 4. GREEN — warm fast, cold fast, within budget.
        self.verdict = VERDICT_GREEN


@dataclass(frozen=True)
class CpythonOracle:
    """Host-native CPython interpreter chosen for the perf floor."""

    cmd: tuple[str, ...]
    executable: str
    version: str
    implementation: str
    sys_platform: str
    machine: str
    arch: str
    pointer_bits: int

    @property
    def display(self) -> str:
        return " ".join(self.cmd)

    def host_metadata(self) -> dict:
        return {
            "cmd": list(self.cmd),
            "executable": self.executable,
            "implementation": self.implementation,
            "version": self.version,
            "sys_platform": self.sys_platform,
            "machine": self.machine,
            "arch": self.arch,
            "pointer_bits": self.pointer_bits,
        }


def _safe_ratio(numerator: float | None, denominator: float | None) -> float | None:
    if numerator is None or denominator is None or denominator <= 0:
        return None
    return round(numerator / denominator, 4)


# ---------------------------------------------------------------------------
# The council's 5-state CLASSIFIER (#69 --classify) — target vs artifact
# ---------------------------------------------------------------------------
#
# Replaces the single warm verdict with the 5 states the council specified. The
# decisive new inputs are machine QUIESCENCE and the repeat-pass CI. The cardinal
# rule: a warm-red is RED_STABLE (a TRUE compiler target) ONLY when it is stable,
# measured quiescent, AND its CI sits clear below 1.00. Any contamination /
# instability / CI-straddle demotes it to RED_NOISY or TIE — the mechanical kill
# for the "0.66 loaded-machine red" class. FAIL_ENGINE maps to RED_STABLE ONLY
# when quiescent+stable; under contamination it is RED_NOISY.


def classify_cell(
    cell: "Cell",
    *,
    quiescent: bool,
    baseline_cell: dict | None = None,
) -> tuple[str, str]:
    """Return (classification, reason) — the 5-state measurement-hygiene verdict.

    ``quiescent`` is the machine-quiescence result for the WHOLE board (a single
    run cannot be partly quiet). ``baseline_cell`` (optional) enables
    DIMENSIONAL_WIN detection: when the warm gate did not flip but a non-warm
    dimension (alloc/RSS/binary-size/cold/compile) improved materially vs the
    baseline. Pure function of the cell's finalized facts + these two inputs;
    unit-tested with synthetic cells (no molt build).
    """
    # Infra states have no warm number to classify — pass them through.
    if cell.verdict in (
        VERDICT_BUILD_FAILED,
        VERDICT_RUN_ERROR,
        VERDICT_RUN_BLOCKED,
        VERDICT_CPY_INCOMPAT,
        VERDICT_FAIL_STALE,
    ):
        return CLASS_INFRA, f"infrastructure verdict {cell.verdict} (no warm number)"

    warm = cell.warm_speedup
    if warm is None:
        return CLASS_INFRA, "no warm_speedup measured"

    # Determine the CI posture if a repeat sweep ran; else fall back to the
    # single-pass robust-stability flag (cell.stable).
    rep = cell.repeat_stability
    has_ci = rep in ("STABLE_BELOW", "STABLE_ABOVE", "STRADDLES")

    # --- TIE: the CI genuinely crosses 1.00 (neither win nor loss) ----------
    # A straddling CI is the council's TIE — even on a quiet machine, the data do
    # not place the cell on one side of the floor. A warm==1.00 single-pass cell
    # (the borderline FAIL_ENGINE ties) is ALSO a TIE under classification: it is
    # not a representational loss, it is statistically CPython. BUT (Rule 4) a
    # warm-tie cell that improved a NON-warm dimension materially vs a baseline is
    # a DIMENSIONAL_WIN, not a bare TIE — the "landed without a warm flip but
    # better elsewhere" lane. So consult the dimensional check before TIE.
    if has_ci and rep == "STRADDLES":
        dim = _dimensional_improvement(cell, baseline_cell)
        if dim is not None:
            return CLASS_DIMENSIONAL_WIN, dim
        return (
            CLASS_TIE,
            f"repeat CI [{cell.repeat_ci_lo}, {cell.repeat_ci_hi}] straddles 1.00 "
            f"(n={cell.repeat_passes}) — neither a win nor a loss",
        )
    if not has_ci and abs(warm - RED_THRESHOLD) < 1e-9:
        dim = _dimensional_improvement(cell, baseline_cell)
        if dim is not None:
            return CLASS_DIMENSIONAL_WIN, dim
        return (
            CLASS_TIE,
            "warm_speedup == 1.00 — statistically CPython (a tie, not a loss); "
            "run --repeat to bound the CI",
        )

    # When a repeat CI exists it is AUTHORITATIVE over the lone point estimate —
    # that is the council's entire reason for --repeat (a flaky point sample must
    # not decide the side of the floor). A STABLE_BELOW CI is a red even if one
    # pass read > 1.0; a STABLE_ABOVE CI is a green even if one pass read < 1.0.
    # Only without a CI does the point estimate decide. (STRADDLES was already
    # routed to TIE above, so here has_ci implies STABLE_BELOW xor STABLE_ABOVE.)
    if has_ci:
        warm_below = rep == "STABLE_BELOW"
        warm_above = rep == "STABLE_ABOVE"
    else:
        warm_below = warm < RED_THRESHOLD
        warm_above = warm > RED_THRESHOLD

    # --- WARM RED branch (CI-governed when present) -------------------------
    if warm_below:
        # RED_STABLE iff: quiescent AND robustly stable AND a repeat CI EXISTS
        # and sits entirely below 1.0. A single-pass red (no CI) is NOT yet
        # RED_STABLE — it is RED_NOISY until --repeat confirms it (a point
        # estimate is not a confidence interval).
        if quiescent and cell.stable and has_ci:
            return (
                CLASS_RED_STABLE,
                f"repeat CI [{cell.repeat_ci_lo}, {cell.repeat_ci_hi}] clears below "
                f"1.00 (point {warm}x), quiescent + stable — TRUE compiler target",
            )
        # Demote to RED_NOISY and NAME why it is not yet a target.
        causes = []
        if not quiescent:
            causes.append("machine NOT quiescent (contaminated)")
        if not cell.stable:
            causes.append("cell unstable (CV/robustness)")
        if not has_ci:
            causes.append("no repeat CI (single pass — run --repeat N to confirm)")
        return (
            CLASS_RED_NOISY,
            f"warm point {warm}x below 1.00 BUT " + "; ".join(causes),
        )

    # --- WARM GREEN / DIMENSIONAL branch (CI-governed when present) ----------
    if warm_above:
        # ASYMMETRY OF CONTAMINATION: a competing build steals cycles from the
        # TIMED molt process, so it can only make molt look SLOWER — never
        # artificially FASTER. Therefore a warm-ABOVE cell under contamination is
        # a CONSERVATIVE green (the quiet number would be >= this); the win is
        # real. Only cell INSTABILITY (CV/robustness) can undermine a green —
        # NOT non-quiescence. (Contrast the warm-BELOW branch, where load CAN
        # manufacture a false red, so quiescence is mandatory there.) Calling a
        # 10x cell RED_NOISY merely because an idle daemon was running is wrong.
        if cell.stable:
            note = f"warm point {warm}x above 1.00, stable"
            if has_ci:
                note += (
                    f", repeat CI [{cell.repeat_ci_lo}, {cell.repeat_ci_hi}] clears "
                    "above 1.00"
                )
            if not quiescent:
                note += (
                    " (measured non-quiescent — contamination is conservative for "
                    "a green: the quiet number can only be faster; board authority "
                    "still gated)"
                )
            else:
                note += " + quiescent"
            return CLASS_GREEN, note
        # warm>1 but the cell is UNSTABLE -> not a confirmed win (a straddle was
        # already caught above; this is point-above with a volatile sample).
        return (
            CLASS_RED_NOISY,
            f"warm point {warm}x above 1.00 BUT cell unstable (CV/robustness) — "
            "green unconfirmed",
        )

    # --- DIMENSIONAL_WIN: warm gate flat, another dimension improved --------
    # Reached only when warm is neither clearly below nor clearly above (e.g. a
    # tie that a baseline shows improved on alloc/RSS/size/cold). Needs a
    # baseline; without one we cannot assert a dimensional improvement.
    dim = _dimensional_improvement(cell, baseline_cell)
    if dim is not None:
        return CLASS_DIMENSIONAL_WIN, dim
    return (
        CLASS_TIE,
        "warm at the 1.00 floor and no material dimensional improvement vs baseline",
    )


def _dimensional_improvement(cell: "Cell", baseline_cell: dict | None) -> str | None:
    """Detect a material non-warm improvement vs a baseline cell (DIMENSIONAL_WIN).

    Compares RSS, binary size, cold_speedup, and compile time against the
    baseline. Returns a human reason naming the improved dimension(s) if ANY beat
    the baseline by >= DIMENSIONAL_WIN_MIN_FRACTION; else None. A warm flip is
    NOT a dimensional win (that is GREEN); this is the "landed without a warm
    flip but materially better elsewhere" lane (Rule 4).
    """
    if not baseline_cell:
        return None
    wins: list[str] = []

    def _improved_lower(new: float | None, old: float | None, label: str) -> None:
        # Lower-is-better dimensions (RSS, size, compile time).
        if new is None or old is None or old <= 0:
            return
        frac = (old - new) / old
        if frac >= DIMENSIONAL_WIN_MIN_FRACTION:
            wins.append(f"{label} {old:.1f}->{new:.1f} (-{frac * 100:.0f}%)")

    def _improved_higher(new: float | None, old: float | None, label: str) -> None:
        # Higher-is-better dimensions (cold_speedup).
        if new is None or old is None or old <= 0:
            return
        frac = (new - old) / old
        if frac >= DIMENSIONAL_WIN_MIN_FRACTION:
            wins.append(f"{label} {old:.2f}->{new:.2f} (+{frac * 100:.0f}%)")

    _improved_lower(
        cell.molt_peak_rss_mib, baseline_cell.get("molt_peak_rss_mib"), "RSS"
    )
    _improved_lower(
        cell.binary_size_kib, baseline_cell.get("binary_size_kib"), "binary"
    )
    _improved_lower(cell.compile_time_s, baseline_cell.get("compile_time_s"), "compile")
    _improved_higher(cell.cold_speedup, baseline_cell.get("cold_speedup"), "cold")
    if not wins:
        return None
    return "warm gate flat, but DIMENSIONAL win: " + "; ".join(wins)


def apply_classification(
    cells: list["Cell"],
    *,
    quiescent: bool,
    baseline_doc: dict | None = None,
) -> None:
    """Set ``classification`` on every cell from quiescence + repeat CI + baseline.

    The single entry point the run/rebuild paths call when --classify is on.
    ``baseline_doc`` (a prior board) enables DIMENSIONAL_WIN; absent, dimensional
    wins are not asserted (they collapse to TIE — conservative by design).
    """
    baseline_cells: dict[str, dict] = {}
    if baseline_doc:
        baseline_cells = {_cell_key(c): c for c in _flatten_cells(baseline_doc)}
    for cell in cells:
        bkey = f"{cell.benchmark} [{cell.backend}/{cell.profile}]"
        cls, reason = classify_cell(
            cell,
            quiescent=quiescent,
            baseline_cell=baseline_cells.get(bkey),
        )
        cell.classification = cls
        cell.classification_reason = reason
        cell.measured_quiescent = quiescent


# --- Triage hints (council "what FACT is missing from IR?" doctrine) --------
# These are NAME-pattern heuristics that point a perf agent at the most likely
# missing IR fact per the council triage doctrine. They are HINTS for the
# summary, never gating logic — the real diagnosis is the per-benchmark
# one-page representation analysis (ruling G).
_FACT_HINTS: tuple[tuple[tuple[str, ...], str], ...] = (
    (
        ("csv", "split", "field", "substr", "slice"),
        "substring/slice repr (alloc-free field extraction)",
    ),
    (
        ("exception", "raise", "try", "except"),
        "zero-cost happy-path exception-state + handler-region ownership",
    ),
    (
        ("etl", "orders", "record", "row"),
        "record/dict value-slot shape + borrow/ownership of stable field flow",
    ),
    (
        ("dict", "set", "map", "counter"),
        "hash-table value-slot Repr + key identity/borrow",
    ),
    (("tuple", "pack", "index"), "tuple element Repr + unboxed-lane stability"),
    (
        ("attr", "method", "dispatch", "class"),
        "class identity/method shape/version guard/call target (devirt)",
    ),
    (
        ("fib", "loop", "sum", "range", "sieve"),
        "induction/range/overflow/lane-stability (counted-loop facts)",
    ),
    (
        ("generator", "gen", "yield", "async", "await", "coro"),
        "frame ownership/resumable-state/fusion",
    ),
    (
        ("bytes", "bytearray", "str", "format"),
        "string/bytes Repr + borrowed-view extraction",
    ),
    (("json", "parse", "roundtrip"), "parse buffer ownership + value-slot Repr"),
)


def _suspect_fact(benchmark: str) -> str:
    name = Path(benchmark).stem.lower()
    for needles, fact in _FACT_HINTS:
        if any(n in name for n in needles):
            return fact
    return "representation/ownership fact (run the one-page diagnosis)"


def _suspect_startup_component(benchmark: str) -> str:
    """Cold-start tax components are program-INDEPENDENT for the fixed floor.

    The cold tax is dominated by the fixed startup cost (process-launch/dyld +
    molt-runtime-init + binary page-in), not the program. The per-benchmark
    delta is binary SIZE (larger linked surface = more page-in) and any extra
    module-init the program's imports trigger. See docs/perf/COLD_START.md.
    """
    name = Path(benchmark).stem.lower()
    if any(n in name for n in ("json", "csv", "import", "etl", "channel", "async")):
        return "module-init + binary page-in (program imports extra stdlib surface)"
    return "fixed startup floor: binary page-in + molt-runtime-init + dyld"


def _cell_from_dict(d: dict) -> Cell:
    """Rehydrate a Cell from a stored per-cell dict (for --rebuild-summary).

    Filters to known dataclass fields so a stored board from an older schema
    still loads; missing fields fall back to the Cell defaults.
    """
    import dataclasses

    known = {f.name for f in dataclasses.fields(Cell)}
    return Cell(**{k: v for k, v in d.items() if k in known})


# ---------------------------------------------------------------------------
# Measurement driver
# ---------------------------------------------------------------------------


def _perfscore_build_env(spec: BackendSpec) -> dict[str, str]:
    """Build the conformance/build env for a backend lane.

    Sets the constitution's session isolation + the LLVM_SYS prefix + the
    MOLT_BACKEND selector. bench._canonical_bench_env folds in the molt
    conformance env (PYTHONPATH, codec, conformance dirs).
    """
    base = os.environ.copy()
    base["MOLT_SESSION_ID"] = PERFSCORE_SESSION_ID
    base["CARGO_TARGET_DIR"] = str(
        REPO_ROOT / "target" / "sessions" / PERFSCORE_SESSION_ID
    )
    if spec.molt_backend is not None:
        base["MOLT_BACKEND"] = spec.molt_backend
    else:
        base.pop("MOLT_BACKEND", None)
    if spec.backend == "llvm":
        prefix = _llvm_sys_prefix()
        if prefix:
            base["LLVM_SYS_211_PREFIX"] = prefix
    env = bench._canonical_bench_env(base)
    return env


def _cpython_run_env() -> dict[str, str]:
    """Env for the CPython baseline — src on PYTHONPATH, deterministic hashing."""
    env = bench._base_python_env()
    env["MOLT_SESSION_ID"] = PERFSCORE_SESSION_ID
    return env


def measure_cell(
    *,
    script_path: Path,
    spec: BackendSpec,
    profile: str,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    batch_server: bench._BenchBatchBuildServer | None,
    cpython_cmd: tuple[str, ...],
    log_dir: Path,
    budget_ms: float | None = None,
    authoritative: bool = True,
    pypy_bin: str | None = None,
    codon_runner: "CodonRunner | None" = None,
    repeat: int = 1,
    emit_cycle_profile: bool = False,
) -> Cell:
    """Build + time one (benchmark, target, backend, profile) cell.

    ``repeat`` (>=1) runs N independent warm measurement PASSES (each a full
    warmup+samples block for molt AND CPython) and attaches a per-pass
    warm_speedup CI; a verdict is STABLE only if the CI does not straddle 1.00.
    ``emit_cycle_profile`` captures a CPU CYCLE profile (``/usr/bin/sample``) for
    a warm-red cell — the Rule-1 attribution signal (cycles, not alloc-count).
    """
    benchmark = bench_suites.canonical_benchmark_key(script_path)
    cell = Cell(
        benchmark=benchmark, target=spec.target, backend=spec.backend, profile=profile
    )
    log_path = log_dir / f"{Path(benchmark).stem}__{spec.backend}__{profile}.log"
    cell.log_artifact = str(log_path.relative_to(REPO_ROOT))
    log_lines: list[str] = [f"# {benchmark} | {spec.backend} | {profile}"]

    build_env = _perfscore_build_env(spec)
    extra_args = bench_suites.molt_args_for_benchmark(script_path)
    build_flag = PROFILE_BUILD_FLAG.get(profile, "release")

    # --- Build the molt binary via the canonical daemon batch build ---------
    binary = None
    if spec.build_target == "native":
        try:
            binary = bench.prepare_molt_binary(
                str(script_path),
                extra_args=extra_args,
                env=build_env,
                build_profile=build_flag,
                batch_server=batch_server,
                build_timeout_s=600.0,
            )
        except Exception as exc:  # noqa: BLE001 - record, never crash the sweep
            log_lines.append(f"BUILD EXCEPTION: {exc!r}")
            binary = None
    else:
        # WASM build/link only — produced via the CLI, not run here.
        binary = _build_wasm_only(script_path, build_env, build_flag, log_lines)

    if not isinstance(binary, bench.MoltBinary):
        cell.build_ok = False
        if isinstance(binary, bench.MoltFailure):
            detail = f" detail={binary.detail}" if binary.detail else ""
            log_lines.append(f"BUILD FAILED status={binary.status}{detail}")
        else:
            log_lines.append("BUILD FAILED")
        cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
        _write_log(log_path, log_lines)
        return cell

    cell.build_ok = True
    cell.binary_size_kib = round(binary.size_kb, 1)
    cell.compile_time_s = round(binary.build_s, 3)
    log_lines.append(
        f"build_ok size_kib={cell.binary_size_kib} compile_s={cell.compile_time_s}"
    )

    if spec.backend in RUN_BLOCKED_BACKENDS:
        cell.run_blocked = True
        cell.run_blocked_reason = (
            "wasm run-path blocked: socket-import instantiation gap (build/link only)"
        )
        log_lines.append(f"RUN-BLOCKED: {cell.run_blocked_reason}")
        cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
        _write_log(log_path, log_lines)
        _release_binary(binary)
        return cell

    run_args = bench.resolve_benchmark_run_args(str(script_path))
    molt_run_env = build_env

    # --- COLD samples (fresh cache; capture stdout once for parity) ---------
    cold_molt = _safe_run_json(
        [str(binary.path), *run_args],
        env=molt_run_env,
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label=f"molt-cold:{benchmark}",
        capture_stdout=True,
    )
    cold_cpy = _safe_run_json(
        [*cpython_cmd, str(script_path), *run_args],
        env=_cpython_run_env(),
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label=f"cpython-cold:{benchmark}",
        capture_stdout=True,
    )

    # --- WARM samples — N independent PASSES (council --repeat N) -----------
    # Each pass is a full warmup+samples block for molt AND CPython, yielding one
    # warm_speedup point estimate. The CANONICAL cell stats come from the pass
    # with the most molt samples (pass 1 in the common case); ALL passes feed the
    # confidence interval. A verdict is STABLE only if that CI clears 1.00.
    n_passes = max(1, repeat)
    per_pass_speedups: list[float] = []
    pass_results: list[tuple[PhaseStats, PhaseStats]] = []
    for pass_idx in range(n_passes):
        for _ in range(warmup):
            _safe_run_json(
                [str(binary.path), *run_args],
                env=molt_run_env,
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=f"molt-warmup:{benchmark}",
            )
        molt_runs = [
            _safe_run_json(
                [str(binary.path), *run_args],
                env=molt_run_env,
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=f"molt-warm:{benchmark}:p{pass_idx}",
            )
            for _ in range(samples)
        ]
        for _ in range(warmup):
            _safe_run_json(
                [*cpython_cmd, str(script_path), *run_args],
                env=_cpython_run_env(),
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=f"cpython-warmup:{benchmark}",
            )
        cpy_runs = [
            _safe_run_json(
                [*cpython_cmd, str(script_path), *run_args],
                env=_cpython_run_env(),
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=f"cpython-warm:{benchmark}:p{pass_idx}",
            )
            for _ in range(samples)
        ]
        m_stats = PhaseStats.from_runs(molt_runs)
        c_stats = PhaseStats.from_runs(cpy_runs)
        pass_results.append((m_stats, c_stats))
        sp = _safe_ratio(c_stats.median_s, m_stats.median_s)
        if sp is not None:
            per_pass_speedups.append(sp)

    # Canonical stats = the pass with the most molt samples (ties -> first).
    molt_stats, cpy_stats = max(
        pass_results, key=lambda pr: (pr[0].n, -pass_results.index(pr))
    )

    # --- CYCLE attribution for warm reds (Rule 1 — cycles, NOT alloc-count) --
    # Capture WHILE the binary still exists. A warm red is warm_speedup <= 1.00
    # i.e. cpython_warm <= molt_warm (the FAIL_ENGINE condition). We profile the
    # running molt binary so the next optimization is steered by CPU self-time,
    # never by an alloc count.
    warm_sp_now = _safe_ratio(cpy_stats.median_s, molt_stats.median_s)
    is_warm_red_now = (
        warm_sp_now is not None
        and warm_sp_now <= RED_THRESHOLD
        and molt_stats.n > 0
        and cpy_stats.n > 0
    )
    if emit_cycle_profile and is_warm_red_now:
        log_lines.append("capturing CYCLE profile (warm red — Rule 1 attribution)")
        cell.cycle_profile = capture_cycle_profile(
            [str(binary.path), *run_args],
            env=molt_run_env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
        )
        avail = cell.cycle_profile.get("available")
        top = cell.cycle_profile.get("top_symbols") or []
        log_lines.append(
            f"cycle_profile available={avail} top={top[0]['symbol'] if top else '-'} "
            f"note={cell.cycle_profile.get('note')}"
        )

    _release_binary(binary)

    cell.molt_stats = asdict(molt_stats)
    cell.cpython_stats = asdict(cpy_stats)
    cell.molt_ok = molt_stats.n > 0 and cold_molt.ok
    cell.cpython_ok = cpy_stats.n > 0 and cold_cpy.ok

    # --- Repeat-pass CI (STABLE only if the CI does not straddle 1.00) -------
    if n_passes > 1 or per_pass_speedups:
        median_sp, var_sp, ci_lo, ci_hi = _warm_speedup_ci(per_pass_speedups)
        cell.repeat_passes = n_passes
        cell.repeat_warm_speedups = [round(s, 4) for s in per_pass_speedups]
        cell.repeat_median_warm = median_sp
        cell.repeat_variance = var_sp
        cell.repeat_ci_lo = ci_lo
        cell.repeat_ci_hi = ci_hi
        cell.repeat_stability = _repeat_stability(ci_lo, ci_hi)

    cell.cold_molt_s = round(cold_molt.elapsed_s, 6) if cold_molt.elapsed_s else None
    cell.cold_cpython_s = round(cold_cpy.elapsed_s, 6) if cold_cpy.elapsed_s else None
    cell.warm_molt_s = molt_stats.median_s
    cell.warm_cpython_s = cpy_stats.median_s

    cell.molt_peak_rss_mib = _max_opt(molt_stats.peak_rss_mib, cold_molt.peak_rss_mib)
    cell.cpython_peak_rss_mib = _max_opt(cpy_stats.peak_rss_mib, cold_cpy.peak_rss_mib)

    # Stability: molt is the ARTIFACT UNDER TEST and MUST be stable. CPython is
    # the reference floor; a single CPython GC/scheduler outlier (common on
    # multi-100ms benchmarks) must NOT invalidate a cell where molt wins
    # decisively and is itself stable — otherwise a won class (e.g.
    # class_hierarchy 8x, molt cv 0.03) gets masked UNSTABLE by one CPython
    # spike. So the cell is trustworthy iff molt is stable AND either CPython is
    # stable OR the warm verdict is ROBUST to CPython's full sample spread (the
    # warm_speedup stays on the same side of the 1.00 floor using BOTH CPython's
    # fastest and slowest sample). This is median-based + outlier-robust per
    # pyperf discipline, never a per-test special case.
    cell.stable = _robust_cell_stable(molt_stats, cpy_stats)

    # One-time output parity (informational; not the gate).
    if cold_molt.stdout is not None and cold_cpy.stdout is not None:
        cell.output_parity = cold_molt.stdout.strip() == cold_cpy.stdout.strip()

    if not cell.molt_ok:
        cell.note = f"molt run unmeasurable (status={cold_molt.status})"
    elif not cell.cpython_ok:
        cell.note = f"cpython run unmeasurable (status={cold_cpy.status})"
    elif cell.stable and not cpy_stats.stable and molt_stats.stable:
        # Transparency: the cell is trusted despite CPython-side jitter because
        # molt is stable AND the verdict is robust to CPython's spread.
        cell.note = (
            f"cpython unstable (cv={cpy_stats.cv}) but verdict robust to its "
            f"spread; molt stable (cv={molt_stats.cv})"
        )

    # --- PyPy comparator lane (informational; never a hard gate) ------------
    # Only measured once per benchmark (lane-independent: PyPy runs the same
    # .py). We attach it to every backend cell for the same benchmark so the
    # column is populated, but it is a CPython-style interpreter comparator,
    # not a molt-backend fact.
    if pypy_bin is not None and cell.warm_molt_s:
        pypy_warm = _measure_interpreter_warm(
            pypy_bin,
            str(script_path),
            run_args,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"pypy:{benchmark}",
        )
        if pypy_warm is not None:
            cell.pypy_warm_s = round(pypy_warm, 6)
            cell.pypy_ratio = _safe_ratio(pypy_warm, cell.warm_molt_s)
            log_lines.append(
                f"pypy warm_median={cell.pypy_warm_s} ratio={cell.pypy_ratio}"
            )

    # --- Codon comparator lane (AOT north star; equivalence-gated) ----------
    if codon_runner is not None and cell.warm_molt_s:
        codon_runner.measure_into(
            cell,
            script_path=script_path,
            run_args=run_args,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            log_lines=log_lines,
        )

    cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
    log_lines.append(
        f"molt warm_median={cell.warm_molt_s} cpython warm_median={cell.warm_cpython_s} "
        f"warm_speedup={cell.warm_speedup} cold_speedup={cell.cold_speedup} "
        f"startup_tax_ms={cell.startup_tax_ms} verdict={cell.verdict}"
    )
    _write_log(log_path, log_lines)
    return cell


def _build_wasm_only(
    script_path: Path,
    build_env: dict[str, str],
    build_flag: str,
    log_lines: list[str],
) -> bench.MoltBinary | None:
    """Build+link a WASM artifact (run-path is blocked; we only verify it links).

    Uses the molt CLI directly with ``--target wasm``. Returns a MoltBinary-like
    handle whose ``path`` is the .wasm and whose size/compile-time are captured,
    so the scoreboard records build-facts for the WASM lane.
    """
    import tempfile

    out_dir = Path(
        tempfile.mkdtemp(prefix="perfscore-wasm-", dir=str(bench.BENCH_TMP_ROOT))
    )
    cmd = [
        *bench._molt_build_cmd(build_flag),
        "--target",
        "wasm",
        "--trusted",
        "--json",
        "--rebuild",
        "--out-dir",
        str(out_dir),
        str(script_path),
    ]
    start = time.perf_counter()
    try:
        res = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_BENCH",
            env=build_env,
            capture_output=True,
            text=True,
            timeout=600.0,
        )
    except Exception as exc:  # noqa: BLE001
        log_lines.append(f"WASM BUILD EXCEPTION: {exc!r}")
        return None
    build_s = time.perf_counter() - start
    if res.returncode != 0:
        tail = (res.stderr or res.stdout or "").strip()[-2000:]
        log_lines.append(f"WASM BUILD FAILED rc={res.returncode}\n{tail}")
        return None
    try:
        payload = json.loads((res.stdout or "{}").strip() or "{}")
    except json.JSONDecodeError:
        log_lines.append("WASM BUILD: non-JSON stdout")
        return None
    out_str = payload.get("data", {}).get("output") or payload.get("output")
    if not out_str:
        # Fall back to scanning the out dir for a .wasm artifact.
        wasms = list(out_dir.rglob("*.wasm"))
        if not wasms:
            log_lines.append("WASM BUILD: no .wasm artifact")
            return None
        out_path = wasms[0]
    else:
        out_path = Path(out_str)
        if not out_path.exists():
            wasms = list(out_dir.rglob("*.wasm"))
            out_path = wasms[0] if wasms else out_path
    if not out_path.exists():
        log_lines.append(f"WASM BUILD: artifact missing {out_path}")
        return None
    size_kb = out_path.stat().st_size / 1024

    class _TmpHolder:
        def cleanup(self) -> None:
            import shutil

            shutil.rmtree(out_dir, ignore_errors=True)

    return bench.MoltBinary(out_path, _TmpHolder(), build_s, size_kb)


def _release_binary(binary: bench.MoltBinary) -> None:
    holder = getattr(binary, "temp_dir", None)
    if holder is not None:
        try:
            holder.cleanup()
        except Exception:  # noqa: BLE001
            pass


def _max_opt(a: float | None, b: float | None) -> float | None:
    vals = [v for v in (a, b) if v is not None]
    return max(vals) if vals else None


def _write_log(path: Path, lines: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


# ---------------------------------------------------------------------------
# PyPy / Codon comparator lanes (council Lane C — best-effort, never a gate)
# ---------------------------------------------------------------------------
#
# PyPy: a DYNAMIC comparator (JIT interpreter). Where PyPy beats molt we NAME
# the missing molt fact; it is NOT a hard gate (PyPy's strength is long-running
# JIT warmup, a different operating point than molt's AOT).
#
# Codon: the AOT NORTH STAR — but Codon is NOT a drop-in for every Python
# program (no full CPython object model, restricted dynamism). A benchmark is
# scored against Codon ONLY if it is on the semantic-equivalence allowlist;
# everything else is recorded "non-equivalent" and NEVER scored win/loss.


def _measure_interpreter_warm(
    interp_bin: str,
    script: str,
    run_args: list[str],
    *,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    label: str,
) -> float | None:
    """Warm steady-state median wall time for a Python-compatible interpreter.

    Same >=samples cold+warm discipline as the CPython path, through safe_run.
    Returns None if the interpreter cannot run the script (so a comparator that
    chokes on a benchmark simply leaves the column null, never poisons a cell).
    """
    env = _cpython_run_env()
    for _ in range(warmup):
        _safe_run_json(
            [interp_bin, script, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"{label}-warmup",
        )
    runs = [
        _safe_run_json(
            [interp_bin, script, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=label,
        )
        for _ in range(samples)
    ]
    stats = PhaseStats.from_runs(runs)
    return stats.median_s if stats.n > 0 else None


# Codon semantic-equivalence allowlist. ONLY these benchmarks are drop-in
# enough that a Codon AOT comparison is meaningful (numeric/loop kernels with
# no CPython-object-model dependence, no dynamic stdlib). A benchmark NOT in
# this set is recorded "non-equivalent" and never scored. Conservative by
# design — false "equivalent" is worse than a missing comparison.
CODON_EQUIVALENT_BENCHMARKS = frozenset(
    {
        "tests/benchmarks/bench_fib.py",
        "tests/benchmarks/bench_sieve.py",
        "tests/benchmarks/bench_sum_loop.py",
        "tests/benchmarks/bench_nbody.py",
        "tests/benchmarks/bench_mandelbrot.py",
        "tests/benchmarks/bench_spectral_norm.py",
        "tests/benchmarks/bench_binary_trees.py",
        "tests/benchmarks/bench_matrix_mul.py",
    }
)


class CodonRunner:
    """Compiles + times a benchmark with Codon when it is on the allowlist.

    Codon is AOT: we compile each allowlisted benchmark to a native binary
    (``codon build -release``) and time it through safe_run with the same
    discipline as molt. A compile failure or a not-allowlisted benchmark
    records the reason and marks the cell ``codon_equivalent`` accordingly —
    it is NEVER scored win/loss.
    """

    def __init__(self, codon_bin: str) -> None:
        self.codon_bin = codon_bin
        self._tmp_root: Path | None = None
        # Codon-compiled binaries link libomp/libcodonrt via @loader_path and
        # need the codon lib dir on DYLD_LIBRARY_PATH to run. Resolve it from
        # the binary location (.../bin/codon -> .../lib/codon).
        self.lib_dir = self._resolve_lib_dir(codon_bin)

    @staticmethod
    def _resolve_lib_dir(codon_bin: str) -> Path | None:
        root = Path(codon_bin).resolve().parent.parent  # .../bin/codon -> root
        cand = root / "lib" / "codon"
        return cand if cand.exists() else None

    def _run_env(self) -> dict[str, str]:
        env = _cpython_run_env()
        if self.lib_dir is not None:
            existing = env.get("DYLD_LIBRARY_PATH", "")
            env["DYLD_LIBRARY_PATH"] = (
                f"{self.lib_dir}:{existing}" if existing else str(self.lib_dir)
            )
            # macOS hardened-runtime strips DYLD_* across some exec boundaries;
            # CODON_LIBRARY is also honored by codon-compiled binaries.
            env.setdefault("CODON_LIBRARY", str(self.lib_dir))
        return env

    def _ensure_tmp(self) -> Path:
        if self._tmp_root is None:
            import tempfile

            self._tmp_root = Path(
                tempfile.mkdtemp(
                    prefix="perfscore-codon-", dir=str(bench.BENCH_TMP_ROOT)
                )
            )
        return self._tmp_root

    def measure_into(
        self,
        cell: "Cell",
        *,
        script_path: Path,
        run_args: list[str],
        samples: int,
        warmup: int,
        rss_mb: int,
        timeout_s: float,
        log_lines: list[str],
    ) -> None:
        key = bench_suites.canonical_benchmark_key(script_path)
        if key not in CODON_EQUIVALENT_BENCHMARKS:
            cell.codon_equivalent = False
            cell.codon_note = "non-equivalent (not on Codon drop-in allowlist)"
            log_lines.append("codon: non-equivalent (not allowlisted) — not scored")
            return
        cell.codon_equivalent = True
        out_bin = self._ensure_tmp() / f"{Path(key).stem}.codon"
        # Codon does not accept arbitrary argv the way CPython does; allowlisted
        # kernels are self-contained (no required run_args). If a benchmark
        # needs args, it should not be on the allowlist.
        build_cmd = [
            self.codon_bin,
            "build",
            "-release",
            "-o",
            str(out_bin),
            str(script_path),
        ]
        try:
            res = harness_memory_guard.guarded_completed_process(
                build_cmd,
                prefix="MOLT_BENCH",
                cwd=REPO_ROOT,
                env=self._run_env(),
                capture_output=True,
                text=True,
                timeout=300,
            )
        except OSError as exc:
            cell.codon_equivalent = False
            cell.codon_note = f"codon build error: {exc!r}"
            log_lines.append(f"codon build EXCEPTION: {exc!r} — not scored")
            return
        if res.timed_out:
            cell.codon_equivalent = False
            cell.codon_note = "codon build timed out after 300s"
            log_lines.append("codon build TIMEOUT — not scored")
            return
        if res.returncode != 0 or not out_bin.exists():
            tail = (res.stderr or res.stdout or "").strip()[-400:]
            # A compile failure does NOT mean molt wins — it means the
            # comparison is unavailable. Record, do not score.
            cell.codon_equivalent = False
            cell.codon_note = f"codon build failed rc={res.returncode}: {tail}"
            log_lines.append(f"codon build FAILED rc={res.returncode} — not scored")
            return
        warm = _measure_codon_warm(
            str(out_bin),
            run_args,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"codon:{key}",
            env=self._run_env(),
        )
        if warm is None:
            cell.codon_note = "codon run unmeasurable"
            log_lines.append("codon run unmeasurable — not scored")
            return
        cell.codon_warm_s = round(warm, 6)
        cell.codon_ratio = _safe_ratio(warm, cell.warm_molt_s)
        cell.codon_note = "equivalent (codon -release AOT)"
        log_lines.append(
            f"codon warm_median={cell.codon_warm_s} ratio={cell.codon_ratio}"
        )

    def close(self) -> None:
        if self._tmp_root is not None:
            import shutil

            shutil.rmtree(self._tmp_root, ignore_errors=True)
            self._tmp_root = None


def _measure_codon_warm(
    codon_bin_path: str,
    run_args: list[str],
    *,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    label: str,
    env: dict[str, str] | None = None,
) -> float | None:
    """Warm median wall time of a compiled Codon binary (safe_run-guarded)."""
    if env is None:
        env = _cpython_run_env()
    for _ in range(warmup):
        _safe_run_json(
            [codon_bin_path, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"{label}-warmup",
        )
    runs = [
        _safe_run_json(
            [codon_bin_path, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=label,
        )
        for _ in range(samples)
    ]
    stats = PhaseStats.from_runs(runs)
    return stats.median_s if stats.n > 0 else None


# ---------------------------------------------------------------------------
# Provenance — the anti-stale-lore enforcement (council ruling A + B)
# ---------------------------------------------------------------------------
#
# Every emitted board carries the exact tree + tool + artifact identity it was
# measured against. If the local HEAD diverges from origin/main the board is
# stamped non-authoritative and the gate refuses (FAIL_STALE) unless the caller
# explicitly opts into local debugging with --allow-nonauthoritative. This is
# the mechanical kill for the "rediscovered a stale-tree failure" class.


def _git_rev() -> str:
    return bench._git_rev() or "unknown"


def _git_output(args: list[str]) -> str | None:
    res = _metadata_probe(["git", *args], cwd=REPO_ROOT, timeout_s=30)
    if res is None:
        return None
    if res.returncode != 0:
        return None
    out = res.stdout.strip()
    return out or None


def _origin_main_sha() -> str | None:
    """SHA of origin/main as the local remote-tracking ref knows it.

    We do NOT fetch here (that is a network side effect the caller controls);
    we report the ref the working tree already has. The Lane-C contract is to
    run inside a worktree freshly checked out at origin/main, so this is the
    authoritative remote tip for the run.
    """
    return _git_output(["rev-parse", "origin/main"])


def _benchmark_tool_identity() -> dict[str, str | None]:
    """Identity of perf_scoreboard.py itself (its own git blob + last commit).

    A board measured by a modified-but-uncommitted tool is as non-authoritative
    as a board measured against a modified tree; we surface both so the reader
    can tell whether the SCOREBOARD LOGIC changed, not just the compiler.
    """
    rel = str(Path(__file__).resolve().relative_to(REPO_ROOT))
    blob = _git_output(["hash-object", str(Path(__file__).resolve())])
    last_commit = _git_output(["log", "-n", "1", "--format=%H", "--", rel])
    # Does the committed blob differ from the on-disk file?
    head_blob = _git_output(["rev-parse", f"HEAD:{rel}"])
    return {
        "path": rel,
        "ondisk_blob_sha": blob,
        "head_blob_sha": head_blob,
        "last_commit_sha": last_commit,
        "modified_vs_head": str(blob is not None and blob != head_blob).lower(),
    }


def _backend_binary_identity_for(spec: "BackendSpec", profile: str) -> str | None:
    """Reuse cli.py's ``_backend_binary_identity`` for the daemon backend binary.

    The backend binary identity (path|mtime_ns|size) is the SAME signal the
    stdlib/TIR caches salt their namespace with; recording it on the board lets
    a reader prove the artifact a number was measured against, and detect the
    stale-cache confound class directly. If cli.py cannot be imported (it is a
    heavy module) we degrade to None rather than fail the board.
    """
    try:
        import molt.cli as _cli  # noqa: PLC0415 - optional, heavy import
    except Exception:  # noqa: BLE001
        return None
    fn = getattr(_cli, "_backend_binary_identity", None)
    if fn is None:
        return None
    backend_bin = _resolve_backend_binary_path(spec, profile)
    if backend_bin is None:
        return None
    try:
        return fn(backend_bin)
    except Exception:  # noqa: BLE001
        return None


def _resolve_backend_binary_path(spec: "BackendSpec", profile: str) -> Path | None:
    """Best-effort path to the daemon's molt-backend binary for this lane.

    The daemon backend is the cargo ``molt-backend`` artifact. Its location
    depends on the active CARGO_TARGET_DIR: it may be the shared
    ``target/<profile_dir>/`` (solo-dev / when CARGO_TARGET_DIR points at
    ``target``) or a session dir ``target/sessions/<id>/<profile_dir>/``. We
    probe, in order: the live ``CARGO_TARGET_DIR``, the perfscore session dir,
    and the shared ``target/`` root — covering every layout cli.py uses.
    release-fast/release-output map to the ``release-fast`` cargo profile dir;
    dev maps to ``debug``. Returns None if nothing is found (the identity then
    degrades to None, never crashes).
    """
    profile_dir = (
        "release-fast" if PROFILE_BUILD_FLAG.get(profile) == "release" else "debug"
    )
    roots: list[Path] = []
    env_target = os.environ.get("CARGO_TARGET_DIR", "").strip()
    if env_target:
        roots.append(Path(env_target))
    roots.append(REPO_ROOT / "target" / "sessions" / PERFSCORE_SESSION_ID)
    roots.append(REPO_ROOT / "target")
    seen: set[Path] = set()
    for root in roots:
        if root in seen:
            continue
        seen.add(root)
        for name in ("molt-backend", "molt"):
            cand = root / profile_dir / name
            if cand.exists():
                return cand
    return None


def _stdlib_cache_key_signal() -> str | None:
    """A stable signal for the stdlib/runtime cache identity.

    The full per-build ``_shared_stdlib_cache_key`` needs the program IR (only
    available mid-build), so for board-level provenance we record the runtime
    backend source-tree fingerprint (``_cache_fingerprint``) which is exactly
    the ``runtime_backend`` component that key is salted with. A reader can
    diff this across boards to know whether the runtime/codegen sources moved.
    """
    try:
        import molt.cli as _cli  # noqa: PLC0415

        fn = getattr(_cli, "_cache_fingerprint", None)
        if fn is not None:
            return str(fn())
    except Exception:  # noqa: BLE001
        return None
    return None


def gather_provenance(
    specs_profiles: list[tuple["BackendSpec", str]] | None = None,
    *,
    quiescence: dict | None = None,
    require_quiescent: bool = False,
) -> dict:
    """Collect the full provenance metadata for a board (council ruling A + #69).

    ``authoritative`` is False whenever the local HEAD diverges from origin/main
    OR the tree is dirty OR the scoreboard tool itself is modified-vs-HEAD — any
    of which means the numbers are not the canonical origin/main contract. When
    ``require_quiescent`` is set, a NON-QUIET machine ALSO forces
    ``authoritative=false`` (#69 Rule 2): a board measured under competing
    build/load is non-authoritative for warm verdicts. The quiescence block is
    ALWAYS recorded (even without --require-quiescent) so a reader can see the
    machine state; the new top-level fields ``active_molt_processes`` /
    ``active_cargo_or_rustc_processes`` / ``loadavg_1m`` / ``ncpu`` /
    ``runnable_signal`` are surfaced for ``--print-provenance``.
    """
    local_head = _git_output(["rev-parse", "HEAD"])
    origin = _origin_main_sha()
    merge_base = (
        _git_output(["merge-base", "HEAD", "origin/main"])
        if origin is not None
        else None
    )
    dirty = bool(_git_output(["status", "--porcelain"]))
    tool = _benchmark_tool_identity()
    tool_modified = tool.get("modified_vs_head") == "true"

    diverges = bool(local_head and origin and local_head != origin)
    quiet = bool(quiescence.get("quiet")) if quiescence else None
    # The quiescence component of authority only BITES when --require-quiescent.
    quiet_blocks = bool(require_quiescent and quiescence is not None and not quiet)
    authoritative = not (diverges or dirty or tool_modified or quiet_blocks)

    backend_identities: dict[str, str | None] = {}
    if specs_profiles:
        for spec, profile in specs_profiles:
            ident = _backend_binary_identity_for(spec, profile)
            backend_identities[f"{spec.backend}/{profile}"] = ident

    prov = {
        "origin_sha": origin,
        "local_head_sha": local_head,
        "merge_base_sha": merge_base,
        "dirty_tree": dirty,
        "diverges_from_origin": diverges,
        "benchmark_tool_sha": tool.get("ondisk_blob_sha"),
        "benchmark_tool_last_commit": tool.get("last_commit_sha"),
        "benchmark_tool_modified": tool_modified,
        "backend_binary_identity": backend_identities,
        "stdlib_cache_key": _stdlib_cache_key_signal(),
        "authoritative": authoritative,
        "authoritative_reason": _authoritative_reason(
            diverges,
            dirty,
            tool_modified,
            quiet_blocks=quiet_blocks,
            quiescence=quiescence,
        ),
        # --- #69 quiescence-guard provenance --------------------------------
        "require_quiescent": require_quiescent,
        "quiescent": quiet,
        "quiescence": quiescence or {},
    }
    if quiescence:
        # Promote the council-mandated NEW fields to the top level of the
        # provenance block (where --print-provenance reads them).
        for k in (
            "active_molt_processes",
            "active_cargo_or_rustc_processes",
            "loadavg_1m",
            "ncpu",
            "runnable_signal",
        ):
            prov[k] = quiescence.get(k)
    return prov


def _authoritative_reason(
    diverges: bool,
    dirty: bool,
    tool_modified: bool,
    *,
    quiet_blocks: bool = False,
    quiescence: dict | None = None,
) -> str:
    if not (diverges or dirty or tool_modified or quiet_blocks):
        return "tree == origin/main, clean, tool unmodified"
    parts = []
    if diverges:
        parts.append("local HEAD diverges from origin/main")
    if dirty:
        parts.append("working tree is dirty")
    if tool_modified:
        parts.append("perf_scoreboard.py modified vs HEAD")
    if quiet_blocks:
        why = "; ".join((quiescence or {}).get("reasons", [])) or "machine not quiet"
        parts.append(f"machine NOT quiescent (--require-quiescent): {why}")
    return "; ".join(parts)


def _refresh_artifact_provenance(provenance: dict, cells: list[Cell]) -> None:
    """Fill in None artifact identities the CURRENT resolver can now compute.

    A stored board may carry ``backend_binary_identity[lane] = None`` (e.g.
    measured before a resolver fix, or the binary was not yet built). On a
    re-derive we upgrade any such None to the now-resolvable identity, keyed by
    the (backend, profile) lanes actually present in the board. The measured
    origin/local/merge-base SHAs are NOT touched. ``stdlib_cache_key`` is
    refreshed only if it is currently null.
    """
    existing = provenance.get("backend_binary_identity")
    if not isinstance(existing, dict):
        existing = {}
    lanes = {(c.backend, c.profile) for c in cells}
    for backend, profile in lanes:
        key = f"{backend}/{profile}"
        if existing.get(key) is None:
            spec = BACKENDS_BY_NAME.get(backend)
            if spec is not None:
                ident = _backend_binary_identity_for(spec, profile)
                if ident is not None:
                    existing[key] = ident
    provenance["backend_binary_identity"] = existing
    if provenance.get("stdlib_cache_key") is None:
        sig = _stdlib_cache_key_signal()
        if sig is not None:
            provenance["stdlib_cache_key"] = sig


def build_scoreboard_doc(
    cells: list[Cell],
    *,
    benchmarks_run: list[str],
    benchmarks_deferred: list[dict],
    cpython_version: str,
    samples: int,
    warmup: int,
    provenance: dict | None = None,
    cpython_identity: dict | None = None,
    pypy_version: str | None = None,
    codon_version: str | None = None,
) -> dict:
    """Assemble the nested machine-readable scoreboard (schema 3).

    Shape: ``benchmark -> target -> backend -> profile -> {cell fields}``.
    Adds the two-dimensional verdict breakdown + the provenance block.
    """
    git_rev = _git_rev()
    nested: dict = {}
    for cell in cells:
        d = asdict(cell)
        (
            nested.setdefault(cell.benchmark, {})
            .setdefault(cell.target, {})
            .setdefault(cell.backend, {})[cell.profile]
        ) = d

    def keys_with(verdict: str) -> list[str]:
        return sorted(_cell_key(asdict(c)) for c in cells if c.verdict == verdict)

    def keys_with_class(cls: str) -> list[str]:
        return sorted(_cell_key(asdict(c)) for c in cells if c.classification == cls)

    # The gate-failing set (the hard reds). FAIL_STALE is conditional (depends
    # on --allow-nonauthoritative), so it is reported separately, not summed in.
    gate_failing = [c for c in cells if c.verdict in GATE_FAILING_VERDICTS]
    stale_cells = [c for c in cells if c.verdict == VERDICT_FAIL_STALE]
    # The 5-state classification is active iff any cell carries a classification
    # (--classify was used). When inactive the breakdown is empty (no-op for the
    # 2-D-only callers).
    classify_active = any(c.classification is not None for c in cells)
    host = {
        "platform": sys.platform,
        "machine": platform.machine(),
        "arch": _host_arch(),
        "pointer_bits": _host_pointer_bits(),
        "python_runner": sys.version.split()[0],
        "cpython_baseline": cpython_version,
        "pypy": pypy_version,
        "codon": codon_version,
    }
    if cpython_identity is not None:
        host["cpython_oracle"] = cpython_identity

    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "cpython_floor_scoreboard",
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": git_rev,
        "provenance": provenance or {},
        "host": host,
        "direction": "speedup = cpython_time / molt_time; >1.0 = molt faster; <=1.0 = engine/cold red",
        "red_threshold": RED_THRESHOLD,
        "unstable_cv_threshold": UNSTABLE_CV,
        "verdict_legend": {
            "GREEN": "warm fast, cold fast, within cold-start budget",
            "FAIL_ENGINE": "warm_speedup <= 1.00 — execution-engine red, RELEASE BLOCKER",
            "FAIL_COLD_BUDGET": "startup_tax_ms > budget_ms — startup regression",
            "WARN_COLD_FLOOR": "cold <= 1.00 but warm > 1.00 & tax within budget — not a hard red",
            "FAIL_STALE": "non-authoritative tree — overrides all (gate fails unless --allow-nonauthoritative)",
            "UNSTABLE": "CV above threshold — untrustworthy in either direction",
            "BUILD_FAILED": "molt build failed",
            "RUN_ERROR": "CPython ran but molt did not",
            "RUN_BLOCKED": "wasm run-path gap (build/link only)",
            "CPY_INCOMPATIBLE": "no CPython floor — excluded from gate",
        },
        "classify_legend": {
            CLASS_RED_STABLE: "warm<1.00, repeat CI clears below 1.00, quiescent+stable — TRUE compiler target",
            CLASS_RED_NOISY: "warm<1.00 BUT contaminated/unstable/CI-straddles — NOT yet a target",
            CLASS_TIE: "repeat CI crosses 1.00 (or warm==1.00) — neither win nor loss",
            CLASS_GREEN: "stable warm>1.00, quiescent — a won class",
            CLASS_DIMENSIONAL_WIN: "warm gate flat but alloc/RSS/size/cold improved >= 5% vs baseline (Rule 4)",
            CLASS_INFRA: "build/run/blocked/cpy-incompat/stale — no warm number to classify",
        },
        "methodology": {
            "samples_per_phase": samples,
            "warmup_runs": warmup,
            "cold_and_warm": True,
            "run_guard": "tools/safe_run.py --json (rss cap + timeout + peak rss)",
            "build": "tools/bench.py daemon batch build (memory-guarded)",
            "warm_speedup": "cpython_warm / molt_warm",
            "cold_speedup": "cpython_cold / molt_cold",
            "startup_tax_ms": "(molt_cold_total - molt_warm_total) * 1000",
        },
        "reserved_columns": {
            "pypy_ratio": (
                f"PyPy {pypy_version}: pypy_warm/molt_warm (>1 = molt faster)"
                if pypy_version
                else "nullable — PyPy not installed"
            ),
            "codon_ratio": (
                f"Codon {codon_version}: codon_warm/molt_warm, equivalence-gated"
                if codon_version
                else "nullable — Codon not installed"
            ),
        },
        "summary": {
            "cells_total": len(cells),
            "cells_green": sum(1 for c in cells if c.verdict == VERDICT_GREEN),
            "cells_unstable": sum(1 for c in cells if c.verdict == VERDICT_UNSTABLE),
            "cells_build_failed": sum(
                1 for c in cells if c.verdict == VERDICT_BUILD_FAILED
            ),
            "cells_run_blocked": sum(
                1 for c in cells if c.verdict == VERDICT_RUN_BLOCKED
            ),
            "cells_error": sum(1 for c in cells if c.verdict == VERDICT_RUN_ERROR),
            "cells_cpython_incompatible": sum(
                1 for c in cells if c.verdict == VERDICT_CPY_INCOMPAT
            ),
            # The two-dimensional verdict counts (the council's gate axes).
            "cells_fail_engine": sum(
                1 for c in cells if c.verdict == VERDICT_FAIL_ENGINE
            ),
            "cells_fail_cold_budget": sum(
                1 for c in cells if c.verdict == VERDICT_FAIL_COLD_BUDGET
            ),
            "cells_warn_cold_floor": sum(
                1 for c in cells if c.verdict == VERDICT_WARN_COLD_FLOOR
            ),
            "cells_fail_stale": len(stale_cells),
            "gate_fails": bool(gate_failing),
            # The two-dimensional breakdown (council ruling A) — every cell
            # keyed by its verdict so a reader routes warm reds to the IR-fact
            # lane and cold reds to the startup lane WITHOUT re-deriving them.
            "verdict_breakdown": {
                "FAIL_ENGINE": keys_with(VERDICT_FAIL_ENGINE),
                "FAIL_COLD_BUDGET": keys_with(VERDICT_FAIL_COLD_BUDGET),
                "WARN_COLD_FLOOR": keys_with(VERDICT_WARN_COLD_FLOOR),
                "FAIL_STALE": keys_with(VERDICT_FAIL_STALE),
                "UNSTABLE": keys_with(VERDICT_UNSTABLE),
                "BUILD_FAILED": keys_with(VERDICT_BUILD_FAILED),
                "RUN_ERROR": keys_with(VERDICT_RUN_ERROR),
                "CPY_INCOMPATIBLE": keys_with(VERDICT_CPY_INCOMPAT),
                "GREEN": keys_with(VERDICT_GREEN),
            },
            # --- #69 5-state classification breakdown (--classify) ----------
            # The measurement-hygiene-aware verdict: TRUE target vs artifact.
            # Empty unless --classify ran. RED_STABLE is the TRUE warm-red set.
            "classify_active": classify_active,
            "cells_red_stable": sum(
                1 for c in cells if c.classification == CLASS_RED_STABLE
            ),
            "cells_red_noisy": sum(
                1 for c in cells if c.classification == CLASS_RED_NOISY
            ),
            "cells_tie": sum(1 for c in cells if c.classification == CLASS_TIE),
            "cells_dimensional_win": sum(
                1 for c in cells if c.classification == CLASS_DIMENSIONAL_WIN
            ),
            "classification_breakdown": {
                CLASS_RED_STABLE: keys_with_class(CLASS_RED_STABLE),
                CLASS_RED_NOISY: keys_with_class(CLASS_RED_NOISY),
                CLASS_TIE: keys_with_class(CLASS_TIE),
                CLASS_GREEN: keys_with_class(CLASS_GREEN),
                CLASS_DIMENSIONAL_WIN: keys_with_class(CLASS_DIMENSIONAL_WIN),
                CLASS_INFRA: keys_with_class(CLASS_INFRA),
            },
        },
        "benchmarks_run": benchmarks_run,
        "benchmarks_deferred": benchmarks_deferred,
        "scoreboard": nested,
    }


def _validate_board_for_emit(doc: dict, *, context: str) -> None:
    problems = validate_board(doc)
    if problems:
        raise ScoreboardSchemaError(context, problems)


def _write_scoreboard_doc(path: Path, doc: dict, *, context: str) -> None:
    _write_scoreboard_doc_atomic(path, doc, context=context)


def _write_scoreboard_doc_atomic(path: Path, doc: dict, *, context: str) -> None:
    _validate_board_for_emit(doc, context=context)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def _print_schema_error(exc: ScoreboardSchemaError) -> None:
    print(f"[schema] {exc.context} FAILED:", file=sys.stderr)
    for problem in exc.problems:
        print(f"    - {problem}", file=sys.stderr)


# ---------------------------------------------------------------------------
# Human-readable summary (gate-failing rows first)
# ---------------------------------------------------------------------------


def print_summary(doc: dict) -> None:
    cells = _flatten_cells(doc)
    by_verdict: dict[str, list[dict]] = {}
    for c in cells:
        by_verdict.setdefault(c.get("verdict", "pending"), []).append(c)

    # --- Authoritative-tree header (council ruling A + B) -------------------
    prov = doc.get("provenance", {})
    authoritative = prov.get("authoritative", True)
    print("\n" + "=" * 100)
    print("CPYTHON FLOOR SCOREBOARD — two-dimensional (warm ≠ cold)")
    print(
        f"  origin/main = {_short(prov.get('origin_sha'))}   "
        f"local HEAD = {_short(prov.get('local_head_sha'))}   "
        f"tool = {_short(prov.get('benchmark_tool_sha'))}"
    )
    print(
        f"  cpython={doc['host']['cpython_baseline']}  "
        f"pypy={doc['host'].get('pypy') or '-'}  "
        f"codon={doc['host'].get('codon') or '-'}"
    )
    # --- Quiescence line (#69 Rule 2) --------------------------------------
    q = prov.get("quiescence") or {}
    if q:
        if q.get("quiet"):
            print(
                f"  QUIESCENT: load={q.get('loadavg_1m')} (<= {q.get('loadavg_threshold')}"
                f", ncpu={q.get('ncpu')})  runnable={q.get('runnable_signal')}  "
                f"builds=0  thermal={'ok' if q.get('thermal_ok') else q.get('thermal_ok')}"
            )
        else:
            print(f"  *** NOT QUIESCENT: {'; '.join(q.get('reasons', []))} ***")
            if prov.get("require_quiescent"):
                print(
                    "      NON-AUTHORITATIVE: machine not quiet; do not optimize "
                    "from this red list (EXPLORATORY only)"
                )
    if authoritative:
        print(
            "  AUTHORITATIVE: tree == origin/main, clean, tool unmodified, machine quiescent"
        )
    else:
        print("  *** WARNING: benchmark is NON-AUTHORITATIVE ***")
        print(f"      reason: {prov.get('authoritative_reason', 'unknown')}")
    print("=" * 100)

    # --- Full table (verdict-ordered) --------------------------------------
    rank = {
        VERDICT_FAIL_STALE: 0,
        VERDICT_FAIL_ENGINE: 1,
        VERDICT_BUILD_FAILED: 1,
        VERDICT_RUN_ERROR: 1,
        VERDICT_UNSTABLE: 2,
        VERDICT_FAIL_COLD_BUDGET: 3,
        VERDICT_WARN_COLD_FLOOR: 4,
        VERDICT_RUN_BLOCKED: 5,
        VERDICT_CPY_INCOMPAT: 5,
        VERDICT_GREEN: 6,
    }
    cells.sort(
        key=lambda c: (
            rank.get(c.get("verdict"), 7),
            -(c.get("warm_speedup") or 0.0),
            c["benchmark"],
        )
    )
    hdr = (
        f"{'VERDICT':<17}{'WARM':>7}  {'COLD':>7}  {'TAXms':>7}  "
        f"{'PYPY':>6}  {'CODON':>6}  {'SIZEKiB':>8}  BENCHMARK [backend/profile]"
    )
    print(hdr)
    print("-" * 100)
    for c in cells:
        print(
            f"{c.get('verdict', '?'):<17}"
            f"{_fmt(c.get('warm_speedup')):>7}  "
            f"{_fmt(c.get('cold_speedup')):>7}  "
            f"{_fmt(c.get('startup_tax_ms'), 0):>7}  "
            f"{_fmt(c.get('pypy_ratio')):>6}  "
            f"{_fmt(c.get('codon_ratio')):>6}  "
            f"{_fmt(c.get('binary_size_kib'), 0):>8}  "
            f"{c['benchmark']} [{c['backend']}/{c['profile']}]"
        )
    print("-" * 100)

    s = doc["summary"]
    print(
        f"TOTAL={s['cells_total']}  GREEN={s['cells_green']}  "
        f"FAIL_ENGINE={s.get('cells_fail_engine', 0)}  "
        f"FAIL_COLD_BUDGET={s.get('cells_fail_cold_budget', 0)}  "
        f"WARN_COLD_FLOOR={s.get('cells_warn_cold_floor', 0)}  "
        f"UNSTABLE={s['cells_unstable']}  BUILD_FAIL={s['cells_build_failed']}  "
        f"RUN_ERROR={s['cells_error']}  CPY_INCOMPAT={s.get('cells_cpython_incompatible', 0)}  "
        f"STALE={s.get('cells_fail_stale', 0)}"
    )

    # --- WARM EXECUTION REDS (the release blockers — IR-fact lane) ---------
    warm_reds = by_verdict.get(VERDICT_FAIL_ENGINE, [])
    print(
        f"\nWARM EXECUTION REDS ({len(warm_reds)}) — execution-engine, RELEASE BLOCKER:"
    )
    print("  (needs an IR FACT, not a local opt — see council triage doctrine)")
    for c in sorted(warm_reds, key=lambda c: c.get("warm_speedup") or 0.0):
        print(
            f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]  -> {c.get('suspected_missing_fact', '?')}"
        )

    # --- COLD-START BUDGET REDS (startup/runtime/artifact lane) -------------
    cold_reds = by_verdict.get(VERDICT_FAIL_COLD_BUDGET, [])
    print(f"\nCOLD-START BUDGET REDS ({len(cold_reds)}) — startup tax over budget:")
    for c in sorted(cold_reds, key=lambda c: -(c.get("startup_tax_ms") or 0.0)):
        print(
            f"    cold={_fmt(c.get('cold_speedup'))}x  tax={_fmt(c.get('startup_tax_ms'), 0)}ms"
            f" (budget {_fmt(c.get('cold_budget_ms'), 0)}ms)  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]  -> {c.get('suspected_startup_component', '?')}"
        )

    # --- WARN_COLD_FLOOR (cold<=1 but warm>1, tax within budget) -----------
    warn_cold = by_verdict.get(VERDICT_WARN_COLD_FLOOR, [])
    if warn_cold:
        print(
            f"\nCOLD-FLOOR WARNINGS ({len(warn_cold)}) — warm>CPython, cold<=CPython "
            "by FIXED startup tax (within budget; NOT a gate fail unless --strict-cold):"
        )
        for c in sorted(warn_cold, key=lambda c: -(c.get("startup_tax_ms") or 0.0))[
            :12
        ]:
            print(
                f"    cold={_fmt(c.get('cold_speedup'))}x  warm={_fmt(c.get('warm_speedup'))}x"
                f"  tax={_fmt(c.get('startup_tax_ms'), 0)}ms  {c['benchmark']} "
                f"[{c['backend']}/{c['profile']}]"
            )
        if len(warn_cold) > 12:
            print(
                f"    ... and {len(warn_cold) - 12} more (full list in JSON verdict_breakdown)"
            )

    # --- BACKEND ERRORS / NON-AUTHORITATIVE --------------------------------
    errs = (
        by_verdict.get(VERDICT_BUILD_FAILED, [])
        + by_verdict.get(VERDICT_RUN_ERROR, [])
        + by_verdict.get(VERDICT_UNSTABLE, [])
    )
    stale = by_verdict.get(VERDICT_FAIL_STALE, [])
    if errs or stale:
        print(f"\nBACKEND ERRORS / NON-AUTHORITATIVE ({len(errs) + len(stale)}):")
        for c in errs:
            origin_rerun = "yes" if not authoritative else "no"
            print(
                f"    {c.get('verdict'):<16} {c['benchmark']} [{c['backend']}/{c['profile']}]"
                f"  stale?={'yes' if not authoritative else 'no'}  "
                f"origin_rerun_needed?={origin_rerun}"
                + (f"  ({c.get('note')})" if c.get("note") else "")
            )
        for c in stale[:5]:
            print(
                f"    FAIL_STALE       {c['benchmark']} [{c['backend']}/{c['profile']}]"
                "  stale?=yes  origin_rerun_needed?=yes"
            )
        if len(stale) > 5:
            print(
                f"    ... and {len(stale) - 5} more stale cells (whole board non-authoritative)"
            )

    # --- REGRESSIONS FROM LAST GREEN (filled by the baseline-diff caller) ---
    regressions = doc.get("_regressions_from_last_green")
    if regressions:
        print(f"\nREGRESSIONS FROM LAST GREEN ({len(regressions)}):")
        for m in regressions:
            print(f"    {m}")

    # --- GREENS WORTH PROTECTING (>2x — do not reopen a won class) ----------
    greens = by_verdict.get(VERDICT_GREEN, [])
    protected = sorted(
        (c for c in greens if (c.get("warm_speedup") or 0.0) > 2.0),
        key=lambda c: -(c.get("warm_speedup") or 0.0),
    )
    print(f"\nGREENS WORTH PROTECTING ({len(protected)}) — won classes, do NOT reopen:")
    for c in protected:
        print(
            f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]"
        )

    # --- 5-STATE CLASSIFICATION (#69 --classify) ---------------------------
    summary = doc.get("summary", {})
    if summary.get("classify_active"):
        by_class: dict[str, list[dict]] = {}
        for c in cells:
            cls = c.get("classification")
            if cls:
                by_class.setdefault(cls, []).append(c)
        cb = summary.get("classification_breakdown", {})
        print(
            f"\n5-STATE CLASSIFICATION (#69): RED_STABLE={len(cb.get(CLASS_RED_STABLE, []))}  "
            f"RED_NOISY={len(cb.get(CLASS_RED_NOISY, []))}  TIE={len(cb.get(CLASS_TIE, []))}  "
            f"GREEN={len(cb.get(CLASS_GREEN, []))}  "
            f"DIMENSIONAL_WIN={len(cb.get(CLASS_DIMENSIONAL_WIN, []))}  "
            f"INFRA={len(cb.get(CLASS_INFRA, []))}"
        )
        red_stable = sorted(
            by_class.get(CLASS_RED_STABLE, []),
            key=lambda c: c.get("warm_speedup") or 0.0,
        )
        print(
            f"\n  TRUE WARM REDS — RED_STABLE ({len(red_stable)}) "
            "[quiescent + stable + CI below 1.00 — the ONLY optimize-from set]:"
        )
        for c in red_stable:
            cp = c.get("cycle_profile") or {}
            top = cp.get("top_symbols") or []
            cyc = (
                f" -> CYCLES top: {top[0]['symbol']}"
                if top
                else (f" -> {cp.get('note')}" if cp.get("note") else "")
            )
            print(
                f"    {_fmt(c.get('warm_speedup'))}x  "
                f"CI=[{_fmt(c.get('repeat_ci_lo'))},{_fmt(c.get('repeat_ci_hi'))}]  "
                f"{c['benchmark']} [{c['backend']}/{c['profile']}]{cyc}"
            )
        noisy = by_class.get(CLASS_RED_NOISY, [])
        if noisy:
            print(
                f"\n  RED_NOISY ({len(noisy)}) — warm<1.00 but contaminated/"
                "unstable/CI-straddles — DO NOT optimize (re-measure quiet):"
            )
            for c in sorted(noisy, key=lambda c: c.get("warm_speedup") or 0.0)[:20]:
                print(
                    f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
                    f"[{c['backend']}/{c['profile']}]  ({c.get('classification_reason')})"
                )
        ties = by_class.get(CLASS_TIE, [])
        if ties:
            print(f"\n  TIE ({len(ties)}) — CI crosses 1.00 (neither win nor loss):")
            for c in sorted(ties, key=lambda c: c["benchmark"])[:20]:
                print(
                    f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
                    f"[{c['backend']}/{c['profile']}]"
                )
        dims = by_class.get(CLASS_DIMENSIONAL_WIN, [])
        if dims:
            print(
                f"\n  DIMENSIONAL_WIN ({len(dims)}) — Rule 4 (no warm flip, dimension improved):"
            )
            for c in dims:
                print(
                    f"    {c['benchmark']} [{c['backend']}/{c['profile']}]  "
                    f"({c.get('classification_reason')})"
                )

    # --- FASTEST NEXT UNLOCK -----------------------------------------------
    unlock = _fastest_next_unlock(warm_reds, cold_reds)
    print(f"\nFASTEST NEXT UNLOCK: {unlock}")

    if doc["benchmarks_deferred"]:
        print(f"\nDEFERRED / CPY-INCOMPATIBLE ({len(doc['benchmarks_deferred'])}):")
        for d in doc["benchmarks_deferred"][:8]:
            print(f"  - {d['benchmark']}: {d['reason']}")
        if len(doc["benchmarks_deferred"]) > 8:
            print(f"  ... and {len(doc['benchmarks_deferred']) - 8} more")
    print("=" * 100 + "\n")


def _short(sha: str | None) -> str:
    if not sha:
        return "-"
    return sha[:12]


def _fastest_next_unlock(warm_reds: list[dict], cold_reds: list[dict]) -> str:
    """One structural fact / one file lane / one gate — the highest-leverage next move.

    Prefer the WORST warm red (engine reds outrank cold reds per ruling A); a
    warm red the most benchmarks share is the fastest class to retire.
    """
    if warm_reds:
        worst = min(warm_reds, key=lambda c: c.get("warm_speedup") or 1e9)
        return (
            f"heal {worst['benchmark']} [{worst['backend']}/{worst['profile']}] "
            f"({_fmt(worst.get('warm_speedup'))}x) — fact: "
            f"{worst.get('suspected_missing_fact', '?')}"
        )
    if cold_reds:
        worst = max(cold_reds, key=lambda c: c.get("startup_tax_ms") or 0.0)
        return (
            f"cold-start: {worst['benchmark']} tax={_fmt(worst.get('startup_tax_ms'), 0)}ms "
            f"— attack {worst.get('suspected_startup_component', '?')}"
        )
    return "no reds — protect the greens; widen the suite for the next class"


def _flatten_cells(doc: dict) -> list[dict]:
    return [dict(cell) for cell in flatten_cells(doc)]


def _fmt(v: float | None, places: int = 2) -> str:
    if v is None:
        return "-"
    if places == 0:
        return f"{v:.0f}"
    return f"{v:.{places}f}"


# ---------------------------------------------------------------------------
# Baseline diff mode
# ---------------------------------------------------------------------------


def diff_against_baseline(
    doc: dict, baseline_path: Path
) -> tuple[list[str], list[str]]:
    """Return (newly_red, regressed_still_green) message lists vs a prior board."""
    try:
        prior = json.loads(baseline_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return ([f"baseline unreadable: {exc}"], [])

    prior_cells = {_cell_key(c): c for c in _flatten_cells(prior)}
    newly_red: list[str] = []
    regressed: list[str] = []
    for c in _flatten_cells(doc):
        key = _cell_key(c)
        old = prior_cells.get(key)
        if old is None:
            continue
        new_ratio = c.get("warm_speedup")
        old_ratio = old.get("warm_speedup")
        # "Newly gating" = a green-or-warn cell that became a hard gate fail.
        was_green = not verdict_fails_gate(str(old.get("verdict", "")))
        now_fails = verdict_fails_gate(
            str(c.get("verdict", "")),
            fail_stale=False,
        )
        if now_fails and was_green:
            newly_red.append(
                f"{key}: NEWLY {c.get('verdict', 'RED')}  "
                f"{_fmt(old_ratio)} -> {_fmt(new_ratio)}"
            )
        elif (
            new_ratio is not None
            and old_ratio is not None
            and not now_fails
            and new_ratio < old_ratio * 0.95  # >5% slower but still passing
        ):
            regressed.append(
                f"{key}: regressed-but-passing  {_fmt(old_ratio)} -> {_fmt(new_ratio)} "
                f"({(new_ratio / old_ratio - 1) * 100:+.1f}%)"
            )
    return newly_red, regressed


def _cell_key(c: dict) -> str:
    return f"{c['benchmark']} [{c['backend']}/{c['profile']}]"


def _latest_baseline(exclude: Path | None = None) -> Path | None:
    """The most recent committed board, EXCLUDING in-progress ``.partial.json``
    checkpoints and an optional explicit path (the board being written now).

    Without the ``.partial`` exclusion a diff would compare a board against its
    own mid-sweep checkpoint (a near-self-diff that hides every regression).
    """
    if not SCOREBOARD_DIR.exists():
        return None
    exclude_resolved = exclude.resolve() if exclude is not None else None
    candidates = [
        p
        for p in sorted(SCOREBOARD_DIR.glob("cpython_*.json"))
        if not p.name.endswith(".partial.json")
        and (exclude_resolved is None or p.resolve() != exclude_resolved)
    ]
    return candidates[-1] if candidates else None


def _gate_exit_code(
    doc: dict,
    *,
    no_gate: bool,
    strict_cold: bool = False,
    allow_nonauthoritative: bool = False,
) -> int:
    """The two-dimensional gate (council ruling A).

    Nonzero iff any FAIL_ENGINE / FAIL_COLD_BUDGET / BUILD_FAILED / RUN_ERROR /
    UNSTABLE. WARN_COLD_FLOOR fails ONLY with ``--strict-cold``. FAIL_STALE
    fails UNLESS ``--allow-nonauthoritative`` (local-debug opt-out). The single
    source of truth shared by run / merge / rebuild-summary.
    """
    if no_gate:
        return 0
    s = doc.get("summary", {})
    if s.get("gate_fails"):
        return 1
    if strict_cold and s.get("cells_warn_cold_floor", 0) > 0:
        return 1
    if (not allow_nonauthoritative) and s.get("cells_fail_stale", 0) > 0:
        return 1
    return 0


def _finalize_with_board_context(
    cells: list[Cell], doc_like: dict, *, allow_nonauthoritative: bool = False
) -> None:
    """Re-finalize stored cells using budgets + the board's own authoritative flag.

    For rebuild/merge we re-run the classifier so a stored board reflects the
    CURRENT verdict logic. The cold-start budget comes from the live budget
    file; the authoritative flag comes from the stored provenance (a stored
    board does not re-derive authoritativeness — it was already stamped).
    ``allow_nonauthoritative`` mirrors the run path: a non-authoritative board's
    cells classify on their REAL numbers (not FAIL_STALE) so a reader can
    re-derive verdicts for local analysis; the board's stored
    ``authoritative=false`` is untouched. We also RE-DERIVE ``stable`` from the
    stored per-runtime stats so a board measured by an older tool picks up the
    current robust-stability rule without re-running any benchmark.
    """
    budgets = _load_cold_start_budgets()
    stored_auth = doc_like.get("provenance", {}).get("authoritative", True)
    effective_auth = stored_auth or allow_nonauthoritative
    for cell in cells:
        # Drop any verdict-DERIVED note (FAIL_STALE / robustness) before
        # re-deriving so a stale note from a prior finalize (e.g. a board that
        # was once stamped FAIL_STALE) does not leak into the new verdict.
        if cell.note in _VERDICT_DERIVED_NOTES or (
            cell.note and cell.note.startswith("non-authoritative tree")
        ):
            cell.note = None
        _rederive_stability(cell)
        cell.finalize(
            budget_ms=_budget_ms_for(budgets, cell.backend, cell.profile),
            authoritative=effective_auth,
        )


def _rederive_stability(cell: Cell) -> None:
    """Recompute ``cell.stable`` from the stored molt/cpython stats dicts.

    ``finalize`` does not recompute stability (it is set at measurement time),
    so a rebuild-summary/merge must re-derive it to apply the current robust
    rule. No-op if the stored stats are absent.
    """
    if not cell.molt_stats or not cell.cpython_stats:
        return
    molt = _phasestats_from_dict(cell.molt_stats)
    cpy = _phasestats_from_dict(cell.cpython_stats)
    if molt is None or cpy is None:
        return
    cell.stable = _robust_cell_stable(molt, cpy)


def _phasestats_from_dict(d: dict) -> PhaseStats | None:
    import dataclasses

    if not isinstance(d, dict):
        return None
    known = {f.name for f in dataclasses.fields(PhaseStats)}
    return PhaseStats(**{k: v for k, v in d.items() if k in known})


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _resolve_benchmark_set(name: str, explicit: list[str] | None) -> list[Path]:
    if explicit:
        return [_resolve_script(b) for b in explicit]
    if name == "smoke":
        keys = bench_suites.SMOKE_BENCHMARKS
    elif name == "core":
        keys = bench_suites.BENCHMARKS
    else:
        raise SystemExit(f"unknown benchmark set: {name}")
    return [_resolve_script(k) for k in keys]


def _resolve_script(key: str) -> Path:
    p = Path(key)
    if p.is_absolute() and p.exists():
        return p
    cand = REPO_ROOT / key
    if cand.exists():
        return cand
    cand2 = REPO_ROOT / "tests" / "benchmarks" / Path(key).name
    if cand2.exists():
        return cand2
    raise SystemExit(f"benchmark not found: {key}")


def _rebuild_summary(
    path: Path,
    *,
    no_gate: bool,
    strict_cold: bool = False,
    allow_nonauthoritative: bool = False,
) -> int:
    """Re-derive a stored board's summary/breakdown/gate from its per-cell data.

    Loads the authoritative per-cell measurements, re-runs them through the
    CURRENT ``build_scoreboard_doc`` (so the summary + verdicts match the
    current tool), writes the board back in place, prints the summary, and
    returns the gate exit code. No binaries are rebuilt; no benchmarks re-run.
    """
    try:
        prior = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"--rebuild-summary: cannot read {path}: {exc}", file=sys.stderr)
        return 2
    cells = [_cell_from_dict(c) for c in _flatten_cells(prior)]
    # Re-run the classifier on the stored measurements so the verdict reflects
    # the CURRENT finalize() logic (the 2-D verdict + budget), not whatever the
    # board was stamped with at measurement time.
    _finalize_with_board_context(
        cells, prior, allow_nonauthoritative=allow_nonauthoritative
    )
    # If the stored board carried the 5-state classification, RE-DERIVE it from
    # the CURRENT classify logic too (reading the measured quiescence from the
    # stored provenance) — so a rebuild picks up a classifier refinement without
    # re-running any benchmark. Only do so when the board was classified (any
    # cell has a classification); else leave the 2-D verdict path untouched.
    if any(c.classification is not None for c in cells):
        stored_q = bool(prior.get("provenance", {}).get("quiescent"))
        apply_classification(cells, quiescent=stored_q, baseline_doc=None)
    method = prior.get("methodology", {})
    # Re-derive the deferred list from cpython-incompatible cells.
    deferred = list(prior.get("benchmarks_deferred", []))
    for cell in cells:
        if cell.verdict == VERDICT_CPY_INCOMPAT:
            dkey = f"{cell.benchmark} [{cell.backend}/{cell.profile}]"
            if not any(d.get("benchmark") == dkey for d in deferred):
                deferred.append(
                    {
                        "benchmark": dkey,
                        "reason": cell.note
                        or "CPython baseline could not run this script",
                    }
                )
    host = prior.get("host", {})
    provenance = dict(prior.get("provenance", {}))
    # Refresh ONLY the artifact identities (backend binary, stdlib cache key) —
    # these are resolvable now (e.g. after a resolver fix) without changing the
    # measured origin/local/merge-base SHAs. A None identity in a stored board
    # that the current resolver CAN fill is upgraded; the measured tree identity
    # is preserved.
    _refresh_artifact_provenance(provenance, cells)
    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=prior.get("benchmarks_run", []),
        benchmarks_deferred=deferred,
        cpython_version=host.get("cpython_baseline", "unknown"),
        samples=method.get("samples_per_phase", DEFAULT_SAMPLES),
        warmup=method.get("warmup_runs", DEFAULT_WARMUP),
        provenance=provenance,
        pypy_version=host.get("pypy"),
        codon_version=host.get("codon"),
    )
    # Preserve the original generation timestamp + git_rev of the measurement.
    doc["generated_at"] = prior.get("generated_at", doc["generated_at"])
    doc["git_rev"] = prior.get("git_rev", doc["git_rev"])
    if "host" in prior:
        doc["host"] = prior["host"]
    try:
        _write_scoreboard_doc(path, doc, context=f"rebuild-summary {path}")
    except ScoreboardSchemaError as exc:
        _print_schema_error(exc)
        return 3
    print(f"[rebuild-summary] rewrote {path}", file=sys.stderr)
    print_summary(doc)
    return _gate_exit_code(
        doc,
        no_gate=no_gate,
        strict_cold=strict_cold,
        allow_nonauthoritative=allow_nonauthoritative,
    )


def _merge_boards(
    sources: list[Path],
    out: Path,
    *,
    no_gate: bool,
    strict_cold: bool = False,
    allow_nonauthoritative: bool = False,
) -> int:
    """Merge per-cell data from multiple scoreboard JSONs into one board.

    Used to combine separately-run backend lanes (e.g. native + llvm) into the
    single ``cpython_<gitrev>.json`` the constitution mandates, without
    re-measuring either lane. Cells are keyed (benchmark, target, backend,
    profile); a later source overrides an earlier one for the same key.
    """
    by_key: dict[tuple, Cell] = {}
    benchmarks_run: list[str] = []
    deferred: list[dict] = []
    host: dict = {}
    method: dict = {}
    provenance: dict = {}
    cpython_version = "unknown"
    git_rev = "unknown"
    generated_at = None
    for src in sources:
        try:
            doc = json.loads(src.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            print(f"--merge: cannot read {src}: {exc}", file=sys.stderr)
            return 2
        host = doc.get("host", host)
        method = doc.get("methodology", method)
        provenance = doc.get("provenance", provenance)
        cpython_version = host.get("cpython_baseline", cpython_version)
        git_rev = doc.get("git_rev", git_rev)
        generated_at = doc.get("generated_at", generated_at)
        for d in _flatten_cells(doc):
            cell = _cell_from_dict(d)
            by_key[(cell.benchmark, cell.target, cell.backend, cell.profile)] = cell
        for b in doc.get("benchmarks_run", []):
            if b not in benchmarks_run:
                benchmarks_run.append(b)
    cells = list(by_key.values())
    _finalize_with_board_context(
        cells,
        {"provenance": provenance},
        allow_nonauthoritative=allow_nonauthoritative,
    )
    for cell in cells:
        if cell.verdict == VERDICT_CPY_INCOMPAT:
            dkey = f"{cell.benchmark} [{cell.backend}/{cell.profile}]"
            if not any(x.get("benchmark") == dkey for x in deferred):
                deferred.append(
                    {
                        "benchmark": dkey,
                        "reason": cell.note or "CPython baseline could not run",
                    }
                )
    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=sorted(benchmarks_run),
        benchmarks_deferred=deferred,
        cpython_version=cpython_version,
        samples=method.get("samples_per_phase", DEFAULT_SAMPLES),
        warmup=method.get("warmup_runs", DEFAULT_WARMUP),
        provenance=provenance,
        pypy_version=host.get("pypy"),
        codon_version=host.get("codon"),
    )
    doc["git_rev"] = git_rev
    if host:
        doc["host"] = host
    if generated_at:
        doc["generated_at"] = generated_at
    try:
        _write_scoreboard_doc(out, doc, context=f"merge {out}")
    except ScoreboardSchemaError as exc:
        _print_schema_error(exc)
        return 3
    print(
        f"[merge] {len(sources)} boards -> {out} ({len(cells)} cells)", file=sys.stderr
    )
    print_summary(doc)
    return _gate_exit_code(
        doc,
        no_gate=no_gate,
        strict_cold=strict_cold,
        allow_nonauthoritative=allow_nonauthoritative,
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="CPython floor-scoreboard — the release-blocking perf gate."
    )
    parser.add_argument(
        "--set",
        default="core",
        choices=["core", "smoke"],
        help="benchmark set (default: core = the curated verified subset)",
    )
    parser.add_argument(
        "--benchmark",
        action="append",
        default=None,
        help="explicit benchmark path/key (repeatable); overrides --set",
    )
    parser.add_argument(
        "--backend",
        action="append",
        default=None,
        choices=["native", "llvm", "wasm"],
        help="backend lane(s) to measure (default: native llvm)",
    )
    parser.add_argument(
        "--profile",
        action="append",
        default=None,
        choices=["release-fast", "release-output", "dev-fast"],
        help="profile(s) to measure (default: release-fast)",
    )
    parser.add_argument(
        "--cpython",
        default=None,
        help="CPython oracle binary (default: system python3, e.g. 3.14 — NOT the venv)",
    )
    parser.add_argument("--samples", type=int, default=DEFAULT_SAMPLES)
    parser.add_argument("--warmup", type=int, default=DEFAULT_WARMUP)
    parser.add_argument("--rss-mb", type=int, default=DEFAULT_RUN_RSS_MB)
    parser.add_argument("--timeout", type=float, default=DEFAULT_RUN_TIMEOUT_S)
    # --- #69 measurement-hygiene flags ------------------------------------
    parser.add_argument(
        "--require-quiescent",
        action="store_true",
        help=(
            "BEFORE measuring, detect contamination (active cargo/rustc/molt "
            "builds [codex excluded], 1-min load > ncpu*0.5, runnable-thread "
            "storm, thermal throttle). A non-quiet machine stamps the board "
            "authoritative=false and prints NON-AUTHORITATIVE; the run still "
            "produces EXPLORATORY numbers, never authoritative warm verdicts."
        ),
    )
    parser.add_argument(
        "--print-provenance",
        action="store_true",
        help=(
            "emit the full provenance block (origin/candidate SHA, dirty, daemon, "
            "stdlib cache key, backend binary identity, cold/warm, repeat/variance "
            "+ the NEW quiescence fields active_molt_processes / "
            "active_cargo_or_rustc_processes / loadavg_1m / ncpu / runnable_signal)"
        ),
    )
    parser.add_argument(
        "--repeat",
        type=int,
        default=1,
        help=(
            "N independent measurement PASSES per cell; compute median + variance "
            "+ a 95%% CI. A verdict is STABLE only if the CI does not straddle "
            "1.00 across passes (default 1 = single pass, no CI)."
        ),
    )
    parser.add_argument(
        "--classify",
        action="store_true",
        help=(
            "replace the single warm verdict with the council's 5 states: "
            "RED_STABLE / RED_NOISY / TIE / GREEN_STABLE / DIMENSIONAL_WIN "
            "(+ INFRA). RED_STABLE (quiescent+stable+CI-below-1.0) is the TRUE "
            "warm-red set. DIMENSIONAL_WIN needs --baseline."
        ),
    )
    parser.add_argument(
        "--emit-cycle-profile",
        action="store_true",
        help=(
            "for warm reds, capture a CYCLE profile (/usr/bin/sample self-time) "
            "and attach the top symbols — the Rule-1 attribution signal (CYCLES, "
            "not alloc-count). Falls back to a documented note if unavailable."
        ),
    )
    # --- #76 warm-hot cycle attribution -----------------------------------
    parser.add_argument(
        "--sample-hot-only",
        action="store_true",
        help=(
            "WARM-HOT cycle attribution (#76): for each benchmark build a LOOPED "
            "(--inner-repeat) + SYMBOLICATED (MOLT_KEEP_SYMBOLS=1) variant, sample "
            "its STEADY STATE, and report the top IN-BINARY hot frames. Defeats "
            "the one-shot launch/page-in (_dyld_start) domination that makes warm "
            "attribution impossible. REFUSES (no hot-path claim) if launch still "
            ">= 40%% of leaf self-time after looping. Writes a JSON profile cell; "
            "does NOT run the speedup gate (use without --classify)."
        ),
    )
    parser.add_argument(
        "--inner-repeat",
        type=int,
        default=DEFAULT_INNER_REPEAT,
        metavar="N",
        help=(
            "inner-repeat factor N for --sample-hot-only: wrap the benchmark "
            "main() in `for _ in range(N): main()` INSIDE one process so launch/"
            f"page-in amortizes (pyperf inner_loops model). Default {DEFAULT_INNER_REPEAT}. "
            "Semantics-preserving (refused if the benchmark is not loopable)."
        ),
    )
    parser.add_argument(
        "--profile-build",
        action="store_true",
        help=(
            "(implied by --sample-hot-only) build benchmarks with molt user-fn "
            "symbols retained (MOLT_KEEP_SYMBOLS=1) so sample/Instruments attribute "
            "to real functions instead of ???. Additive: never changes the normal "
            "stripped product build or any speedup measurement."
        ),
    )
    parser.add_argument(
        "--out",
        default=None,
        help="output JSON path (default: bench/scoreboard/cpython_<gitrev>.json)",
    )
    parser.add_argument(
        "--baseline",
        nargs="?",
        const="__latest__",
        default=None,
        help="diff against a prior scoreboard JSON (default: latest in bench/scoreboard/)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="tiny 1-benchmark x 1-backend run to prove the pipeline + schema",
    )
    parser.add_argument(
        "--no-gate",
        action="store_true",
        help="always exit 0 (measure-only; do not fail CI on RED)",
    )
    parser.add_argument(
        "--strict-cold",
        action="store_true",
        help="make WARN_COLD_FLOOR fail the gate too (default: cold-floor warns only)",
    )
    parser.add_argument(
        "--allow-nonauthoritative",
        action="store_true",
        help=(
            "permit a non-authoritative tree (local != origin/main, or dirty) to "
            "run + not auto-fail the gate via FAIL_STALE — for LOCAL DEBUGGING. "
            "The board is still stamped authoritative=false."
        ),
    )
    parser.add_argument(
        "--pypy",
        nargs="?",
        const="__auto__",
        default=None,
        help="add a PyPy comparator lane (path, or bare flag to auto-detect pypy3.11/3.10)",
    )
    parser.add_argument(
        "--codon",
        nargs="?",
        const="__auto__",
        default=None,
        help="add a Codon AOT comparator lane (path, or bare flag to auto-detect ~/.codon)",
    )
    parser.add_argument(
        "--rebuild-summary",
        default=None,
        help=(
            "re-derive the summary/breakdown/gate from a stored scoreboard's "
            "per-cell data (no re-measurement); writes back in place and "
            "re-applies the gate. Keeps a committed board consistent with the "
            "current tool without rebuilding any binary."
        ),
    )
    parser.add_argument(
        "--merge",
        nargs="+",
        default=None,
        metavar="SRC.json",
        help=(
            "merge per-cell data from multiple scoreboard JSONs into --out "
            "(combine separately-run backend lanes; no re-measurement)"
        ),
    )
    ns = parser.parse_args(argv)

    if ns.rebuild_summary is not None:
        return _rebuild_summary(
            Path(ns.rebuild_summary),
            no_gate=ns.no_gate,
            strict_cold=ns.strict_cold,
            allow_nonauthoritative=ns.allow_nonauthoritative,
        )

    if ns.merge is not None:
        merge_out = (
            Path(ns.out) if ns.out else SCOREBOARD_DIR / f"cpython_{_git_rev()}.json"
        )
        return _merge_boards(
            [Path(p) for p in ns.merge],
            merge_out,
            no_gate=ns.no_gate,
            strict_cold=ns.strict_cold,
            allow_nonauthoritative=ns.allow_nonauthoritative,
        )

    backends = ns.backend or ["native", "llvm"]
    profiles = ns.profile or ["release-fast"]

    if ns.self_test:
        ns.set = "smoke"
        ns.benchmark = ["tests/benchmarks/bench_fib.py"]
        backends = ["native"]
        profiles = ["release-fast"]
        ns.samples = max(2, min(ns.samples, 3))
        ns.warmup = 1
        print("[self-test] bench_fib x native x release-fast, samples=%d" % ns.samples)

    scripts = _resolve_benchmark_set(ns.set, ns.benchmark)
    try:
        cpython_oracle = _resolve_system_cpython(ns.cpython)
    except RuntimeError as exc:
        print(f"[scoreboard] {exc}", file=sys.stderr)
        return 2
    cpython_version = cpython_oracle.version
    cpython_identity = cpython_oracle.host_metadata()
    print(
        "[scoreboard] CPython oracle: "
        f"{cpython_oracle.display} "
        f"({cpython_oracle.version}, {cpython_oracle.sys_platform}/"
        f"{cpython_oracle.arch}, {cpython_oracle.pointer_bits}-bit)",
        file=sys.stderr,
    )

    # --- Quiescence guard (#69 Rule 2) — measure BEFORE timing -------------
    # Detect contamination first so a non-quiet machine is stamped
    # authoritative=false (when --require-quiescent) BEFORE any number is taken.
    quiescence = gather_quiescence()
    if not quiescence["quiet"]:
        print(
            "[scoreboard] machine NOT quiescent — " + "; ".join(quiescence["reasons"]),
            file=sys.stderr,
        )
        if ns.require_quiescent:
            print(
                "[scoreboard] *** NON-AUTHORITATIVE: machine not quiet; do not "
                "optimize from this red list (EXPLORATORY only) ***",
                file=sys.stderr,
            )
    else:
        print(
            f"[scoreboard] machine quiescent (load={quiescence['loadavg_1m']} "
            f"ncpu={quiescence['ncpu']} runnable={quiescence['runnable_signal']} "
            f"builds=0)",
            file=sys.stderr,
        )

    # --- #76 WARM-HOT cycle attribution path (looped + symbolicated) -------
    # A self-contained profiling path: for each benchmark build a looped +
    # symbolicated variant, sample its steady state, apply the refusal gate, and
    # write a JSON profile (the cycle facts). It is NOT a speedup measurement and
    # never runs the release gate — so it returns here before the timing sweep.
    if ns.sample_hot_only:
        if len(backends) != 1:
            print(
                "[hot-only] one backend per run; pass exactly one --backend "
                f"(got {backends}) — defaulting to {backends[0]}",
                file=sys.stderr,
            )
        spec = BACKENDS_BY_NAME[backends[0]]
        profile = profiles[0]
        if ns.inner_repeat < 2:
            print(
                f"[hot-only] --inner-repeat={ns.inner_repeat} < 2 (nothing to "
                "amortize); refusing.",
                file=sys.stderr,
            )
            return 2
        hot_cells = run_hot_only_profiles(
            scripts=scripts,
            spec=spec,
            profile=profile,
            inner_loops=ns.inner_repeat,
            rss_mb=ns.rss_mb,
        )
        return _emit_hot_only_board(
            hot_cells,
            spec=spec,
            profile=profile,
            inner_loops=ns.inner_repeat,
            quiescence=quiescence,
            cpython_version=cpython_version,
            out=ns.out,
        )

    # --- Provenance + authoritative gate (council ruling A + B + #69) ------
    specs_profiles = [(BACKENDS_BY_NAME[b], p) for b in backends for p in profiles]
    provenance = gather_provenance(
        specs_profiles,
        quiescence=quiescence,
        require_quiescent=ns.require_quiescent,
    )
    # provenance.authoritative records the TRUTH (tree==origin, clean, tool
    # unmodified). `--allow-nonauthoritative` does NOT change that truth — it
    # lets the cells classify on their REAL numbers (not FAIL_STALE) for local
    # debugging, while the board still records authoritative=false and the gate
    # is told not to auto-fail on staleness.
    authoritative = bool(provenance.get("authoritative", True))
    effective_authoritative = authoritative or ns.allow_nonauthoritative
    if not authoritative:
        print(
            "[scoreboard] *** WARNING: local tree diverges from origin; benchmark "
            "is non-authoritative unless explicitly requested ***",
            file=sys.stderr,
        )
        print(
            f"[scoreboard]     reason: {provenance.get('authoritative_reason')}",
            file=sys.stderr,
        )
        if ns.allow_nonauthoritative:
            print(
                "[scoreboard]     --allow-nonauthoritative: classifying real "
                "numbers; board stays authoritative=false; gate will NOT "
                "FAIL_STALE.",
                file=sys.stderr,
            )
        else:
            print(
                "[scoreboard]     (the gate will FAIL_STALE; pass "
                "--allow-nonauthoritative to run for local debugging)",
                file=sys.stderr,
            )

    # --- PyPy / Codon comparator lanes (council Lane C) --------------------
    pypy_bin = _resolve_pypy(ns.pypy) if ns.pypy is not None else None
    pypy_version = _probe_interp_version(pypy_bin) if pypy_bin else None
    if pypy_bin:
        print(
            f"[scoreboard] PyPy comparator: {pypy_bin} ({pypy_version})",
            file=sys.stderr,
        )
    codon_bin = _resolve_codon(ns.codon) if ns.codon is not None else None
    codon_runner = CodonRunner(codon_bin) if codon_bin else None
    codon_version = _probe_codon_version(codon_bin) if codon_bin else None
    if codon_bin:
        print(
            f"[scoreboard] Codon comparator: {codon_bin} ({codon_version})",
            file=sys.stderr,
        )

    budgets = _load_cold_start_budgets()

    git_rev = _git_rev()
    SCOREBOARD_DIR.mkdir(parents=True, exist_ok=True)
    out_path = Path(ns.out) if ns.out else SCOREBOARD_DIR / f"cpython_{git_rev}.json"
    log_dir = SCOREBOARD_DIR / f"logs_{git_rev}"
    partial_path = out_path.with_suffix(".partial.json")

    benchmarks_run: list[str] = []
    benchmarks_deferred: list[dict] = []
    cells: list[Cell] = []

    # Per (backend, profile) we open ONE daemon batch build server and reuse it
    # across the whole benchmark set — matching bench.py's amortized-build model.
    for backend_name in backends:
        spec = BACKENDS_BY_NAME[backend_name]
        for profile in profiles:
            cell_budget_ms = _budget_ms_for(budgets, backend_name, profile)
            batch_server = None
            if spec.build_target == "native":
                try:
                    batch_server = bench._BenchBatchBuildServer(
                        _perfscore_build_env(spec)
                    )
                except Exception as exc:  # noqa: BLE001
                    print(
                        f"[warn] could not start batch build server for "
                        f"{backend_name}/{profile}: {exc!r}; falling back to per-build",
                        file=sys.stderr,
                    )
                    batch_server = None
            try:
                for script in scripts:
                    key = bench_suites.canonical_benchmark_key(script)
                    print(
                        f"[scoreboard] {key} | {backend_name} | {profile} ...",
                        file=sys.stderr,
                        flush=True,
                    )
                    cell = measure_cell(
                        script_path=script,
                        spec=spec,
                        profile=profile,
                        samples=ns.samples,
                        warmup=ns.warmup,
                        rss_mb=ns.rss_mb,
                        timeout_s=ns.timeout,
                        batch_server=batch_server,
                        cpython_cmd=cpython_oracle.cmd,
                        log_dir=log_dir,
                        budget_ms=cell_budget_ms,
                        authoritative=effective_authoritative,
                        pypy_bin=pypy_bin,
                        codon_runner=codon_runner,
                        repeat=ns.repeat,
                        emit_cycle_profile=ns.emit_cycle_profile,
                    )
                    cells.append(cell)
                    if key not in benchmarks_run:
                        benchmarks_run.append(key)
                    # CPython-incompatible benchmarks have no valid floor and
                    # are excluded from the gate — record the exclusion
                    # explicitly (no silent truncation).
                    if cell.verdict == VERDICT_CPY_INCOMPAT:
                        dkey = f"{key} [{backend_name}/{profile}]"
                        if not any(d["benchmark"] == dkey for d in benchmarks_deferred):
                            benchmarks_deferred.append(
                                {
                                    "benchmark": dkey,
                                    "reason": cell.note
                                    or "CPython baseline could not run this script",
                                }
                            )
                    # Checkpoint partial JSON after every cell (death-recoverable).
                    try:
                        _checkpoint(
                            partial_path,
                            cells,
                            benchmarks_run,
                            benchmarks_deferred,
                            cpython_version,
                            ns.samples,
                            ns.warmup,
                            provenance=provenance,
                            cpython_identity=cpython_identity,
                            pypy_version=pypy_version,
                            codon_version=codon_version,
                        )
                    except ScoreboardSchemaError as exc:
                        _print_schema_error(exc)
                        return 3
                    print(
                        f"    -> {cell.verdict}  warm={_fmt(cell.warm_speedup)} "
                        f"cold={_fmt(cell.cold_speedup)} tax={_fmt(cell.startup_tax_ms, 0)}ms",
                        file=sys.stderr,
                        flush=True,
                    )
            finally:
                if batch_server is not None:
                    try:
                        batch_server.close()
                    except Exception:  # noqa: BLE001
                        pass
    if codon_runner is not None:
        codon_runner.close()

    # --- 5-state classification (#69 --classify) ---------------------------
    # Set after the whole sweep so the WHOLE-board quiescence + each cell's
    # repeat CI are both available. DIMENSIONAL_WIN needs the baseline board.
    if ns.classify:
        baseline_doc = None
        if ns.baseline is not None:
            bpath = (
                _latest_baseline(exclude=out_path)
                if ns.baseline == "__latest__"
                else Path(ns.baseline)
            )
            if bpath is not None and bpath.exists():
                try:
                    baseline_doc = json.loads(bpath.read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError):
                    baseline_doc = None
        apply_classification(
            cells, quiescent=bool(quiescence["quiet"]), baseline_doc=baseline_doc
        )

    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=benchmarks_run,
        benchmarks_deferred=benchmarks_deferred,
        cpython_version=cpython_version,
        samples=ns.samples,
        warmup=ns.warmup,
        provenance=provenance,
        cpython_identity=cpython_identity,
        pypy_version=pypy_version,
        codon_version=codon_version,
    )
    if ns.print_provenance:
        _print_provenance(provenance)
    # Attach the regressions-from-last-green list so print_summary can surface
    # it in the classified output (council ruling A section).
    doc["_out_path"] = str(out_path)
    _attach_regressions(doc)
    doc.pop("_out_path", None)
    try:
        _write_scoreboard_doc(out_path, doc, context=f"scoreboard {out_path}")
    except ScoreboardSchemaError as exc:
        _print_schema_error(exc)
        return 3
    if partial_path.exists():
        partial_path.unlink()
    print(f"\nscoreboard JSON -> {out_path}", file=sys.stderr)

    if ns.self_test:
        # The self-test PROVES the pipeline + schema, not the perf/stale gate.
        # It inherently dirties the tree (the tool under test is modified), so
        # subjecting it to FAIL_STALE would be circular — it validates the
        # SCHEMA and returns on that alone.
        problems = validate_board(doc)
        print_summary(doc)
        if problems:
            print("[self-test] SCHEMA VALIDATION FAILED:", file=sys.stderr)
            for p in problems:
                print(f"    - {p}", file=sys.stderr)
            return 3
        print(
            "[self-test] schema OK: required top-level keys + per-cell fields present, "
            "2-D verdict + provenance + gate wired, JSON round-trips.",
            file=sys.stderr,
        )
        return 0

    print_summary(doc)

    if ns.baseline is not None:
        baseline_path = (
            _latest_baseline(exclude=out_path)
            if ns.baseline == "__latest__"
            else Path(ns.baseline)
        )
        if baseline_path is None or not baseline_path.exists():
            print("[baseline] no prior scoreboard to diff against.", file=sys.stderr)
        else:
            newly_red, regressed = diff_against_baseline(doc, baseline_path)
            print(f"\n[baseline diff vs {baseline_path.name}]")
            if newly_red:
                print("  NEWLY GATING:")
                for m in newly_red:
                    print(f"    {m}")
            if regressed:
                print("  REGRESSED (still passing):")
                for m in regressed:
                    print(f"    {m}")
            if not newly_red and not regressed:
                print("  no new reds, no regressions.")

    return _gate_exit_code(
        doc,
        no_gate=ns.no_gate,
        strict_cold=ns.strict_cold,
        allow_nonauthoritative=ns.allow_nonauthoritative,
    )


def _proc_summary(procs: object) -> str:
    """One-line summary of a build-process list (pid:exe pairs)."""
    if not isinstance(procs, list) or not procs:
        return "0"
    parts = []
    for p in procs[:6]:
        if isinstance(p, dict):
            cmd = (p.get("cmd") or "").split()
            parts.append(f"{p.get('pid')}:{cmd[0] if cmd else '?'}")
    suffix = ", ".join(parts)
    extra = f" (+{len(procs) - 6} more)" if len(procs) > 6 else ""
    return f"{len(procs)} [{suffix}{extra}]"


def _print_provenance(provenance: dict) -> None:
    """Emit the FULL provenance block (#69 --print-provenance).

    Prints every field a reader needs to certify (or reject) a board's
    authority: the origin/candidate SHAs, dirty/daemon/cache identity, the
    cold/warm + repeat/variance posture, AND the council's NEW quiescence
    fields (``active_molt_processes``, ``active_cargo_or_rustc_processes``,
    ``loadavg_1m``, ``ncpu``, ``runnable_signal``). This is the human-auditable
    twin of the JSON provenance block; it does not re-measure anything.
    """
    p = provenance
    q = p.get("quiescence") or {}
    print("\n" + "=" * 100)
    print("PROVENANCE (full) — #69 measurement-hygiene block")
    print("=" * 100)
    # --- Tree / artifact identity (council ruling A) -----------------------
    print("  [tree identity]")
    print(f"    origin_sha (origin/main)     = {p.get('origin_sha')}")
    print(f"    candidate_sha (local HEAD)   = {p.get('local_head_sha')}")
    print(f"    merge_base_sha               = {p.get('merge_base_sha')}")
    print(f"    dirty_tree                   = {p.get('dirty_tree')}")
    print(f"    diverges_from_origin         = {p.get('diverges_from_origin')}")
    print(f"    benchmark_tool_sha (on-disk) = {p.get('benchmark_tool_sha')}")
    print(f"    benchmark_tool_last_commit   = {p.get('benchmark_tool_last_commit')}")
    print(f"    benchmark_tool_modified      = {p.get('benchmark_tool_modified')}")
    print("  [backend_binary_identity (daemon / stale-cache guard)]")
    bbi = p.get("backend_binary_identity") or {}
    if bbi:
        for lane, ident in sorted(bbi.items()):
            print(f"    {lane:<24} = {ident}")
    else:
        print("    (none recorded)")
    print(f"    stdlib_cache_key             = {p.get('stdlib_cache_key')}")
    # --- Quiescence (#69 Rule 2) — the NEW fields, named explicitly ---------
    print("  [quiescence (#69 Rule 2)]")
    print(f"    require_quiescent            = {p.get('require_quiescent')}")
    print(f"    quiescent                    = {p.get('quiescent')}")
    print(
        f"    active_molt_processes        = {_proc_summary(p.get('active_molt_processes'))}"
    )
    print(
        "    active_cargo_or_rustc_processes = "
        f"{_proc_summary(p.get('active_cargo_or_rustc_processes'))}"
    )
    print(f"    loadavg_1m                   = {p.get('loadavg_1m')}")
    print(f"    loadavg_threshold            = {q.get('loadavg_threshold')}")
    print(f"    ncpu                         = {p.get('ncpu')}")
    print(f"    runnable_signal              = {p.get('runnable_signal')}")
    print(
        f"    thermal_ok                   = {q.get('thermal_ok')}  ({q.get('thermal_note')})"
    )
    if q.get("reasons"):
        print(f"    NON-QUIET reasons            = {'; '.join(q.get('reasons', []))}")
    # --- Authority verdict --------------------------------------------------
    print("  [authority]")
    print(f"    authoritative                = {p.get('authoritative')}")
    print(f"    authoritative_reason         = {p.get('authoritative_reason')}")
    print("=" * 100)


def _attach_regressions(doc: dict) -> None:
    """Compute REGRESSIONS FROM LAST GREEN vs the latest committed board.

    Surfaced in print_summary's classified output. Best-effort: a missing/older
    baseline simply leaves the section empty. Excludes the board being written
    now (``_out_path``) AND any ``.partial.json`` so it never near-self-diffs.
    """
    out_path = doc.get("_out_path")
    baseline = _latest_baseline(exclude=Path(out_path) if out_path else None)
    if baseline is None or not baseline.exists():
        return
    _newly, regressed = diff_against_baseline(doc, baseline)
    if regressed:
        doc["_regressions_from_last_green"] = regressed


def _checkpoint(
    path: Path,
    cells: list[Cell],
    benchmarks_run: list[str],
    benchmarks_deferred: list[dict],
    cpython_version: str,
    samples: int,
    warmup: int,
    *,
    provenance: dict | None = None,
    cpython_identity: dict | None = None,
    pypy_version: str | None = None,
    codon_version: str | None = None,
) -> None:
    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=benchmarks_run,
        benchmarks_deferred=benchmarks_deferred,
        cpython_version=cpython_version,
        samples=samples,
        warmup=warmup,
        provenance=provenance,
        cpython_identity=cpython_identity,
        pypy_version=pypy_version,
        codon_version=codon_version,
    )
    _write_scoreboard_doc_atomic(path, doc, context=f"checkpoint {path}")


def _resolve_pypy(arg: str) -> str | None:
    """Resolve a PyPy interpreter path (explicit, or auto-detect 3.11/3.10)."""
    import shutil

    if arg and arg != "__auto__":
        return arg if Path(arg).exists() else shutil.which(arg)
    for cand in (
        "/opt/homebrew/bin/pypy3.11",
        "/opt/homebrew/bin/pypy3.10",
        "/opt/homebrew/bin/pypy3",
        shutil.which("pypy3.11") or "",
        shutil.which("pypy3.10") or "",
        shutil.which("pypy3") or "",
    ):
        if cand and Path(cand).exists():
            return cand
    return None


def _resolve_codon(arg: str) -> str | None:
    """Resolve a Codon binary path (explicit, or auto-detect ~/.codon/bin)."""
    import shutil

    if arg and arg != "__auto__":
        return arg if Path(arg).exists() else shutil.which(arg)
    default = Path.home() / ".codon" / "bin" / "codon"
    if default.exists():
        return str(default)
    return shutil.which("codon")


def _probe_interp_version(interp_bin: str | None) -> str | None:
    if not interp_bin:
        return None
    res = _metadata_probe([interp_bin, "--version"], timeout_s=30)
    if res is None:
        return None
    out = (res.stdout or res.stderr or "").strip().splitlines()
    return out[0].replace("Python ", "") if out else None


def _probe_codon_version(codon_bin: str | None) -> str | None:
    if not codon_bin:
        return None
    res = _metadata_probe([codon_bin, "--version"], timeout_s=30)
    if res is None:
        return None
    out = (res.stdout or res.stderr or "").strip()
    return f"codon {out}" if out else None


_CPYTHON_IDENTITY_PROBE = "\n".join(
    [
        "import json",
        "import platform",
        "import struct",
        "import sys",
        "print(json.dumps({",
        "    'implementation': platform.python_implementation(),",
        "    'version': platform.python_version(),",
        "    'executable': sys.executable,",
        "    'sys_platform': sys.platform,",
        "    'machine': platform.machine(),",
        "    'pointer_bits': struct.calcsize('P') * 8,",
        "}, sort_keys=True))",
    ]
)


def _resolve_system_cpython(explicit: str | None) -> CpythonOracle:
    """Resolve the host-native CPython oracle used as the performance floor."""

    candidates = [(explicit,)] if explicit else _default_cpython_candidate_cmds()
    failures: list[str] = []
    for raw_cmd in candidates:
        oracle, reason = _probe_cpython_candidate(raw_cmd)
        if oracle is not None:
            return oracle
        if reason:
            failures.append(f"{_format_cmd(raw_cmd)}: {reason}")

    source = f"explicit --cpython {explicit!r}" if explicit else "default CPython"
    target = f"{sys.platform}/{_host_arch()}/{_host_pointer_bits()}-bit"
    detail = "; ".join(failures[:8]) if failures else "no candidates found"
    if len(failures) > 8:
        detail += f"; ... {len(failures) - 8} more rejected"
    raise RuntimeError(f"could not resolve {source} oracle for {target}: {detail}")


def _probe_cpython_candidate(
    raw_cmd: tuple[str, ...],
) -> tuple[CpythonOracle | None, str]:
    try:
        probe_cmd = _canonical_interpreter_cmd(raw_cmd)
    except (FileNotFoundError, OSError) as exc:
        return None, str(exc)

    res = _metadata_probe([*probe_cmd, "-c", _CPYTHON_IDENTITY_PROBE], timeout_s=30)
    if res is None:
        return None, "identity probe failed"
    if res.returncode != 0:
        return None, f"identity probe exited {res.returncode}: {_probe_tail(res)}"

    lines = [line for line in (res.stdout or "").splitlines() if line.strip()]
    if not lines:
        return None, "identity probe emitted no JSON"
    try:
        payload = json.loads(lines[-1])
    except json.JSONDecodeError as exc:
        return None, f"identity probe emitted invalid JSON: {exc}"

    implementation = str(payload.get("implementation", ""))
    version = str(payload.get("version", ""))
    executable = str(payload.get("executable", ""))
    sys_platform = str(payload.get("sys_platform", ""))
    machine = str(payload.get("machine", ""))
    arch = _normalize_arch(machine)
    try:
        pointer_bits = int(payload.get("pointer_bits"))
    except (TypeError, ValueError):
        return None, f"invalid pointer width {payload.get('pointer_bits')!r}"

    if implementation != "CPython":
        return None, f"implementation is {implementation!r}, not CPython"
    if _python_version_key(version) < (3, 12, 0):
        return None, f"CPython {version} is below the 3.12 floor"
    if sys_platform != sys.platform:
        return None, f"platform {sys_platform!r} != host {sys.platform!r}"
    if arch != _host_arch():
        return None, f"arch {arch!r} != host {_host_arch()!r}"
    if pointer_bits != _host_pointer_bits():
        return None, f"pointer width {pointer_bits} != host {_host_pointer_bits()}"
    if _is_project_managed_interpreter(executable):
        return None, f"project-managed interpreter cannot be baseline: {executable}"

    try:
        runtime_executable = bench._canonical_interpreter(executable)
    except (FileNotFoundError, OSError) as exc:
        return None, f"probed executable is not runnable: {exc}"

    return (
        CpythonOracle(
            cmd=(runtime_executable,),
            executable=runtime_executable,
            version=version,
            implementation=implementation,
            sys_platform=sys_platform,
            machine=machine,
            arch=arch,
            pointer_bits=pointer_bits,
        ),
        "",
    )


def _default_cpython_candidate_cmds() -> list[tuple[str, ...]]:
    """Return host-aware CPython candidates, newest first, no project venvs."""

    candidates: list[tuple[str, ...]] = []

    def add(cmd: tuple[str, ...]) -> None:
        if cmd and cmd not in candidates:
            candidates.append(cmd)

    def add_path_name(name: str) -> None:
        for path in _path_executable_candidates(name):
            if not _is_project_managed_interpreter(path):
                add((path,))

    if sys.platform == "darwin":
        for path in (
            "/opt/homebrew/bin/python3",
            "/usr/local/bin/python3",
            "/opt/homebrew/bin/python3.14",
            "/usr/local/bin/python3.14",
            "/opt/homebrew/bin/python3.13",
            "/usr/local/bin/python3.13",
            "/opt/homebrew/bin/python3.12",
            "/usr/local/bin/python3.12",
        ):
            add((path,))

    for name in ("python3.14", "python3.13", "python3.12", "python3", "python"):
        add_path_name(name)

    if os.name == "nt":
        for launcher in _path_executable_candidates("py"):
            for version in ("-3.14", "-3.13", "-3.12"):
                add((launcher, version))
    else:
        for path in (
            "/usr/local/bin/python3.14",
            "/usr/local/bin/python3.13",
            "/usr/local/bin/python3.12",
            "/usr/local/bin/python3",
            "/usr/bin/python3.14",
            "/usr/bin/python3.13",
            "/usr/bin/python3.12",
            "/usr/bin/python3",
        ):
            add((path,))

    return candidates


def _path_executable_candidates(name: str) -> list[str]:
    path = Path(name)
    if path.is_absolute() or path.parent != Path("."):
        return [name]

    suffixes = [""]
    if os.name == "nt" and not path.suffix:
        suffixes = [
            ext.lower()
            for ext in os.environ.get("PATHEXT", ".COM;.EXE;.BAT;.CMD").split(
                os.pathsep
            )
            if ext
        ]

    out: list[str] = []
    seen: set[str] = set()
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        if not directory:
            continue
        for suffix in suffixes:
            candidate = Path(directory) / f"{name}{suffix}"
            key = str(candidate).lower()
            if key in seen:
                continue
            seen.add(key)
            if candidate.is_file():
                out.append(str(candidate))
    return out


def _canonical_interpreter_cmd(raw_cmd: tuple[str, ...]) -> tuple[str, ...]:
    if not raw_cmd or not raw_cmd[0]:
        raise FileNotFoundError("empty CPython candidate command")
    return (bench._canonical_interpreter(raw_cmd[0]), *raw_cmd[1:])


def _is_project_managed_interpreter(path: str) -> bool:
    normalized = path.replace("\\", "/").lower()
    return (
        "/.venv/" in normalized
        or "/target/sessions/" in normalized
        or "/sessions/" in normalized
    )


def _normalize_arch(machine: str) -> str:
    normalized = machine.strip().lower().replace("-", "_").replace(" ", "")
    if normalized in {"amd64", "x64", "x86_64"}:
        return "x86_64"
    if normalized in {"arm64", "aarch64"}:
        return "aarch64"
    if normalized in {"i386", "i486", "i586", "i686", "x86"}:
        return "x86"
    return normalized or "unknown"


def _host_arch() -> str:
    return _normalize_arch(platform.machine())


def _host_pointer_bits() -> int:
    return 64 if sys.maxsize > 2**32 else 32


def _python_version_key(version: str) -> tuple[int, int, int]:
    parts: list[int] = []
    for raw in version.split(".")[:3]:
        digits = "".join(ch for ch in raw if ch.isdigit())
        parts.append(int(digits) if digits else 0)
    while len(parts) < 3:
        parts.append(0)
    return (parts[0], parts[1], parts[2])


def _format_cmd(cmd: tuple[str, ...]) -> str:
    return " ".join(cmd)


def _probe_tail(res: subprocess.CompletedProcess[str]) -> str:
    lines = [
        line.strip()
        for text in (res.stdout, res.stderr)
        for line in (text or "").splitlines()
        if line.strip()
    ]
    return " | ".join(lines[-2:])[:240] or "no output"


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
