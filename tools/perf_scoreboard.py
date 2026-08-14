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

import datetime as dt
import json
import os
import platform
import re
import shlex
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict
from pathlib import Path

import perf_scoreboard_cli as _perf_scoreboard_cli
from perf_scoreboard_model import (
    BACKENDS_BY_NAME as BACKENDS_BY_NAME,
    COLD_START_BUDGET_PATH as COLD_START_BUDGET_PATH,
    DEFAULT_RUN_RSS_MB as DEFAULT_RUN_RSS_MB,
    DEFAULT_RUN_TIMEOUT_S as DEFAULT_RUN_TIMEOUT_S,
    DEFAULT_SAMPLES as DEFAULT_SAMPLES,
    DEFAULT_WARMUP as DEFAULT_WARMUP,
    DIMENSIONAL_WIN_MIN_FRACTION as DIMENSIONAL_WIN_MIN_FRACTION,
    NATIVE_CRANELIFT as NATIVE_CRANELIFT,
    NATIVE_LLVM as NATIVE_LLVM,
    NON_AUTHORITATIVE_NOTE as NON_AUTHORITATIVE_NOTE,
    PERFSCORE_SESSION_ID as PERFSCORE_SESSION_ID,
    PROFILE_BUILD_FLAG as PROFILE_BUILD_FLAG,
    REPO_ROOT as REPO_ROOT,
    RUN_BLOCKED_BACKENDS as RUN_BLOCKED_BACKENDS,
    SAFE_RUN as SAFE_RUN,
    SAFE_RUN_POLL_S as SAFE_RUN_POLL_S,
    SCOREBOARD_DIR as SCOREBOARD_DIR,
    SRC_ROOT as SRC_ROOT,
    TOOLS_ROOT as TOOLS_ROOT,
    WASM as WASM,
    BackendSpec as BackendSpec,
    Cell as Cell,
    CpythonOracle as CpythonOracle,
    PhaseStats as PhaseStats,
    RunOutcome as RunOutcome,
    ScoreboardSchemaError as ScoreboardSchemaError,
    _apply_perf_attribution as _apply_perf_attribution,
    _budget_ms_for as _budget_ms_for,
    _cell_from_dict as _cell_from_dict,
    _dimensional_improvement as _dimensional_improvement,
    _llvm_sys_prefix as _llvm_sys_prefix,
    _load_cold_start_budgets as _load_cold_start_budgets,
    _parse_safe_run_line as _parse_safe_run_line,
    _repeat_stability as _repeat_stability,
    _robust_cell_stable as _robust_cell_stable,
    _safe_ratio as _safe_ratio,
    _safe_run_json as _safe_run_json,
    _suspect_startup_component as _suspect_startup_component,
    _tail_text as _tail_text,
    _warm_speedup_ci as _warm_speedup_ci,
    classify_cell as classify_cell,
)


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
    flatten_cells as flatten_cells,
    validate_board as validate_board,
    verdict_fails_gate as verdict_fails_gate,
)

from perf_scoreboard_measure import (  # noqa: E402
    _build_wasm_only as _build_wasm_only,
    _cpython_run_env as _cpython_run_env,
    _max_opt as _max_opt,
    _measure_codon_warm as _measure_codon_warm,
    _measure_interpreter_warm as _measure_interpreter_warm,
    _perfscore_build_env as _perfscore_build_env,
    _release_binary as _release_binary,
    _write_log as _write_log,
    measure_cell as measure_cell,
)
from perf_scoreboard_profile import (  # noqa: E402
    DEFAULT_INNER_REPEAT as DEFAULT_INNER_REPEAT,
    HOT_SAMPLE_WARMUP_S as HOT_SAMPLE_WARMUP_S,
    HOT_SAMPLE_WINDOW_S as HOT_SAMPLE_WINDOW_S,
    LAUNCH_DOMINANCE_REFUSAL_FRACTION as LAUNCH_DOMINANCE_REFUSAL_FRACTION,
    MOLT_KEEP_SYMBOLS_ENV as MOLT_KEEP_SYMBOLS_ENV,
    _LAUNCH_FRAMES as _LAUNCH_FRAMES,
    _emit_hot_only_board as _emit_hot_only_board,
    _is_launch_frame as _is_launch_frame,
    _parse_sample_heaviest as _parse_sample_heaviest,
    _profiling_tmp_root as _profiling_tmp_root,
    _shquote as _shquote,
    _terminate as _terminate,
    _time_one_run as _time_one_run,
    build_profiling_binary as build_profiling_binary,
    capture_hot_only_profile as capture_hot_only_profile,
    run_hot_only_profiles as run_hot_only_profiles,
    classify_launch_dominance as classify_launch_dominance,
    top_in_binary_frames as top_in_binary_frames,
)
from perf_scoreboard_report import (  # noqa: E402
    _cell_key as _cell_key,
    _checkpoint as _checkpoint,
    _fastest_next_unlock as _fastest_next_unlock,
    _finalize_with_board_context as _finalize_with_board_context,
    _flatten_cells as _flatten_cells,
    _fmt as _fmt,
    _gate_exit_code as _gate_exit_code,
    _latest_baseline as _latest_baseline,
    _phasestats_from_dict as _phasestats_from_dict,
    _print_provenance as _print_provenance,
    _print_schema_error as _print_schema_error,
    _proc_summary as _proc_summary,
    _rederive_stability as _rederive_stability,
    _short as _short,
    _validate_board_for_emit as _validate_board_for_emit,
    _write_scoreboard_doc as _write_scoreboard_doc,
    _write_scoreboard_doc_atomic as _write_scoreboard_doc_atomic,
    diff_against_baseline as diff_against_baseline,
    print_summary as print_summary,
)
from perf_scoreboard_resolver import (  # noqa: E402
    _canonical_interpreter_cmd as _canonical_interpreter_cmd,
    _format_cmd as _format_cmd,
    _is_project_managed_interpreter as _is_project_managed_interpreter,
    _normalize_arch as _normalize_arch,
    _path_executable_candidates as _path_executable_candidates,
    _probe_codon_version as _probe_codon_version,
    _probe_interp_version as _probe_interp_version,
    _probe_tail as _probe_tail,
    _python_version_key as _python_version_key,
    _resolve_codon as _resolve_codon,
    _resolve_pypy as _resolve_pypy,
)


# The constitution's session isolation (must be set before any build command).

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
# ``pgrep -fl`` uses these only as a broad discovery filter; the final decision
# is argv/image-aware so proof-log observers do not count as build workers just
# because a log path contains "cargo" or "rustc".
_BUILD_PROC_PATTERNS = ("cargo", "rustc", "molt-backend", "molt build")
_BUILD_IMAGE_NAMES = frozenset(
    {
        "cargo",
        "cargo.exe",
        "rustc",
        "rustc.exe",
        "molt-backend",
        "molt-backend.exe",
    }
)
_MOLT_CLI_IMAGE_NAMES = frozenset({"molt", "molt.exe"})
_PYTHON_IMAGE_NAMES = frozenset(
    {"python", "python.exe", "python3", "python3.exe", "py", "py.exe"}
)
_OBSERVER_IMAGE_NAMES = frozenset(
    {
        "tail",
        "tail.exe",
        "grep",
        "grep.exe",
        "findstr",
        "findstr.exe",
    }
)
_CODEX_EXCLUDE = "codex"  # never counted/killed (project policy)
# Dimensional-win materiality gate: a non-warm-flip improvement must beat the
# baseline by at least this fraction on a dimension (alloc/RSS/size/cold) to be
# called a DIMENSIONAL_WIN rather than noise. 5% is the smallest delta that
# survives run-to-run measurement jitter on these dimensions.

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
# Default inner-repeat factor when --sample-hot-only is requested without an
# explicit --inner-repeat. 40 amortizes a ~0.6s launch over dozens of
# steady-state bodies (a multi-second process for the curated benchmarks) while
# staying inside a sane RSS budget. The refusal gate below enforces that this
# was enough (raise it if launch still dominates); if a benchmark amplifies a
# per-iteration leak past the RSS cap at this N, the OOM-refusal says to LOWER
# it. (Tuned: at N=40 bench_etl_orders ~2.6s/2.0GiB, bench_exception_heavy
# ~3.8s; both yield launch=0% in-binary=100% steady-state leaderboards.)
# The CPU sample warmup (seconds) BEFORE the sampler attaches, so the first
# iterations (cold I-cache, first-touch page-in) are not in the steady window.
# The steady-state sampling window (seconds); auto-fitted DOWN to the looped
# process's remaining lifetime so the sampler always closes before exit.
# REFUSAL gate (same fail-closed discipline as #69's quiescence guard): if
# launch/page-in still accounts for >= this fraction of leaf self-time AFTER
# inner-repeat + symbols, the loop factor was too small — the attribution is
# INVALID and the tool refuses to emit a hot-path claim ("increase
# --inner-repeat") rather than report a launch-dominated leaderboard as if it
# were the program's hot path.
# Leaf-self-time frames that are PROCESS LAUNCH / PAGE-IN, not program work.
# ``_dyld_start`` is the dynamic-loader entry (launch + first-touch page-in of
# the static binary); the dyld TLV bootstrap and the launch-time image-load
# msg traps are the same class. Matched (symbol, lib) so a same-named program
# symbol is never mis-counted as launch.

# Notes that are DERIVED from a verdict (vs measurement notes). A re-derive
# (rebuild-summary/merge) clears these so a stale verdict-note can't leak.


# safe_run RSS cap + wall-clock timeout per run. Generous enough for the heavy
# benchmarks (class_hierarchy, bytes_find @ 2s CPython) without letting a
# runaway reach OOM territory.

# WASM cannot be run on this host today (socket-import instantiation gap); we
# build+link only and mark the run-path blocked. Luau has its own harness.


# ---------------------------------------------------------------------------
# Backend / profile descriptors
# ---------------------------------------------------------------------------

# CLI --build-profile value -> the cargo profile it resolves to (for the
# scoreboard label). "release" is the daily contract profile and maps to the
# release-fast cargo profile for the backend (see cli.py:_backend_profile).


# ---------------------------------------------------------------------------
# Run timing via safe_run.py (RSS cap + timeout + peak-RSS, for EVERY binary)
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# Statistics — median, stdev, coefficient of variation, stability
# ---------------------------------------------------------------------------

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


def _windows_process_snapshot() -> list[dict[str, object]] | None:
    """Bounded Win32 process snapshot with command lines."""

    if os.name != "nt":
        return None
    script = (
        "Get-CimInstance Win32_Process | "
        "Select-Object ProcessId,Name,CommandLine | ConvertTo-Json -Compress"
    )
    res = _metadata_probe(
        [
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ],
        timeout_s=15,
    )
    if res is None or res.returncode != 0:
        return None
    text = (res.stdout or "").strip()
    if not text:
        return []
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        return None
    rows = payload if isinstance(payload, list) else [payload]
    return [row for row in rows if isinstance(row, dict)]


def _split_process_args(cmd: str) -> list[str]:
    try:
        return shlex.split(cmd, posix=os.name != "nt")
    except ValueError:
        return [part for part in re.split(r"\s+", cmd.strip()) if part]


def _process_arg_basename(arg: str) -> str:
    stripped = arg.strip().strip("'\"").rstrip("\\/")
    return stripped.replace("\\", "/").rsplit("/", 1)[-1].lower()


def _is_build_process_command(cmd: str, *, image_name: str | None = None) -> bool:
    image = _process_arg_basename(image_name or "")
    if image in _OBSERVER_IMAGE_NAMES:
        return False
    if image in _BUILD_IMAGE_NAMES:
        return True

    args = _split_process_args(cmd)
    bases = [_process_arg_basename(arg) for arg in args]
    if any(base in _BUILD_IMAGE_NAMES for base in bases):
        return True

    for idx, base in enumerate(bases):
        if (
            base in _MOLT_CLI_IMAGE_NAMES
            and idx + 1 < len(args)
            and args[idx + 1] == "build"
        ):
            return True
        if (
            base in _PYTHON_IMAGE_NAMES
            and idx + 3 < len(args)
            and args[idx + 1] == "-m"
            and args[idx + 2] == "molt"
            and args[idx + 3] == "build"
        ):
            return True
    return False


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
    if os.name == "nt":
        rows = _windows_process_snapshot()
        if rows is None:
            return [
                {
                    "pid": -1,
                    "cmd": "windows-process-probe-unavailable",
                    "probe_failed": True,
                }
            ]
        out: list[dict] = []
        for row in rows:
            try:
                pid = int(row.get("ProcessId", -1))
            except (TypeError, ValueError):
                continue
            image_name = str(row.get("Name") or "")
            cmd = str(row.get("CommandLine") or image_name)
            cmdl = cmd.lower()
            if not cmd or _CODEX_EXCLUDE in cmdl or pid in (self_pid, parent_pid):
                continue
            if _is_build_process_command(cmd, image_name=image_name):
                out.append({"pid": pid, "cmd": cmd})
        return out
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
        if _is_build_process_command(cmd):
            out.append({"pid": pid, "cmd": cmd})
    return out


def _windows_cpu_load_as_loadavg() -> float | None:
    """Return Windows CPU load normalized onto the loadavg threshold scale."""

    if os.name != "nt":
        return None
    script = (
        "(Get-CimInstance Win32_Processor | "
        "Measure-Object -Property LoadPercentage -Average).Average"
    )
    res = _metadata_probe(
        [
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ],
        timeout_s=10,
    )
    if res is None or res.returncode != 0:
        return None
    lines = [line.strip() for line in (res.stdout or "").splitlines() if line.strip()]
    if not lines:
        return None
    try:
        pct = float(lines[-1])
    except ValueError:
        return None
    ncpu = _ncpu()
    if ncpu is None:
        return None
    return max(0.0, pct) / 100.0 * float(ncpu)


def _loadavg_1m() -> float | None:
    """1-minute load average from the host OS.

    Prefer Python's portable Unix binding (Linux/macOS runners), then fall back
    to the macOS ``sysctl`` spelling. On Windows, use a bounded CPU-load CIM
    probe normalized to the same ``ncpu * QUIESCENT_LOAD_FRACTION`` scale.
    """
    if os.name == "nt":
        return _windows_cpu_load_as_loadavg()
    getloadavg = getattr(os, "getloadavg", None)
    if getloadavg is not None:
        try:
            values = getloadavg()
            return float(values[0]) if values else None
        except (AttributeError, OSError, TypeError, ValueError, IndexError):
            pass
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
    count = os.cpu_count()
    if isinstance(count, int) and count > 0:
        return count
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


# ---------------------------------------------------------------------------
# Cycle attribution (#69 Rule 1 + --emit-cycle-profile) — CYCLES, not allocs
# ---------------------------------------------------------------------------
#
# Rule 1: alloc-attribution alone CANNOT justify a warm-time opt. A warm red
# needs CYCLE attribution. For each warm red we capture a CYCLE profile (macOS
# ``/usr/bin/sample``) of the running molt binary and attach the top symbols.
# This is the signal the NEXT optimization is steered by — never the alloc count.


# Quiescence wait (#69 Rule 2): same predicate, bounded decay window.
def wait_for_quiescence(
    *,
    timeout_s: float,
    poll_s: float = 15.0,
    sleep_fn=time.sleep,
    emit_wait=None,
) -> dict:
    """Return a quiescence sample, waiting briefly for transient load decay.

    The predicate is still exactly ``gather_quiescence()``. This helper only
    prevents a just-finished build from forcing an immediate non-authoritative
    board while the load signal decays. If the host stays noisy through the
    budget, the final noisy sample is returned and the caller still fails
    closed under ``--require-quiescent``.
    """
    timeout = max(0.0, float(timeout_s))
    poll = max(0.1, float(poll_s))
    waited = 0.0
    attempts = 0

    while True:
        sample = gather_quiescence()
        attempts += 1
        sample["quiescence_wait_attempts"] = attempts
        sample["quiescence_waited_s"] = round(waited, 3)
        sample["quiescence_wait_timeout_s"] = timeout
        if sample.get("quiet") or waited >= timeout:
            return sample

        sleep_s = min(poll, timeout - waited)
        if sleep_s <= 0:
            return sample
        if emit_wait is not None:
            emit_wait(sample, attempts, sleep_s, timeout - waited)
        sleep_fn(sleep_s)
        waited += sleep_s


# Cycle attribution (#69 Rule 1): CYCLES, not alloc counts.
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


# ---------------------------------------------------------------------------
# Warm-hot cycle attribution (#76): inner-repeat + symbolicate + hot-only sample
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# One scoreboard cell: benchmark x target x backend x profile
# ---------------------------------------------------------------------------

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


# --- Derived warm-red attribution (cycle facts before taxonomy fallback) -----


# ---------------------------------------------------------------------------
# Measurement driver
# ---------------------------------------------------------------------------


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


# ---------------------------------------------------------------------------
# Provenance — the anti-stale-lore enforcement (council ruling A + B)
# ---------------------------------------------------------------------------
#
# Every emitted board carries the exact tree + tool + machine + artifact
# identity it was measured against. If that provenance is not canonical (tree
# drift, dirty tree, modified tool, or required quiescence failure), the board is
# stamped non-authoritative and the gate refuses (FAIL_STALE) unless the caller
# explicitly opts into local debugging with --allow-nonauthoritative. This is
# the mechanical kill for stale or contaminated performance evidence.


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
    path = Path(__file__).resolve()
    rel = path.relative_to(REPO_ROOT).as_posix()
    blob = _git_output(["hash-object", f"--path={rel}", str(path)])
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


def _refuses_nonauthoritative_measurement(
    *, authoritative: bool, allow_nonauthoritative: bool
) -> bool:
    """Whether a release scoreboard must stop before expensive measurement."""
    return not authoritative and not allow_nonauthoritative


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
            "FAIL_STALE": "non-authoritative board — overrides all (gate fails unless --allow-nonauthoritative)",
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


# ---------------------------------------------------------------------------
# Human-readable summary (gate-failing rows first)
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Baseline diff mode
# ---------------------------------------------------------------------------


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
    return _perf_scoreboard_cli.main(globals(), argv)


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


def _host_arch() -> str:
    return _normalize_arch(platform.machine())


def _host_pointer_bits() -> int:
    return 64 if sys.maxsize > 2**32 else 32


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
